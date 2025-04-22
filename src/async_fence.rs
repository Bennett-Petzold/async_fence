/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

/*! Defines the core async wait for a waker. */

use core::{
    hash::Hash,
    mem::MaybeUninit,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};

#[cfg(feature = "alloc")]
use core::ops::{Deref, DerefMut};

use spin::mutex::SpinMutex;

#[cfg(feature = "alloc")]
use crate::extending_arr::WakerArrExtending;

#[cfg(feature = "alloc")]
extern crate alloc;

pub type FenceWaker = MaybeUninit<Waker>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct FenceQueue<Arr: AsMut<[FenceWaker]>> {
    data: Arr,
    pos: usize,
}

impl<Arr> FenceQueue<Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Creates a new shared queue.
    ///
    /// # Safety
    /// The queue *must* be entirely filled as [`MaybeUninit::uninit`].
    pub const unsafe fn new(data: Arr) -> Self {
        Self { data, pos: 0 }
    }
}

/// Asynchronous fence.
///
/// [`Self::wait`] creates futures that delay until a [`Self::hold`] handle is
/// dropped (released). Both types of handles borrow from the fence -- it acts
/// as a context and must be kept alive for full fencing interaction.
///
/// Use [`Self::new_arr`] for statically sized fences. Use
/// [`Self::default`] for dynamically sized fences.
///
/// # Example
/// ```
/// use async_fence::{StaticFence};
///
/// const FENCE_LEN: usize = 3;
///
/// // This fence will live for the entire program.
/// static FENCE: StaticFence<FENCE_LEN> = StaticFence::new_arr();
///
/// let holder = FENCE.hold();
///
/// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// rt.block_on(async {
///     let handles: [_; 3] = core::array::from_fn(|_| tokio::spawn(FENCE.wait()));
///
///     // After the holder is dropped, all the waiters finish.
///     drop(holder);
///     for handle in handles {
///         handle.await;
///     }
/// });
/// ```
#[derive(Debug)]
pub struct Fence<Arr: AsMut<[FenceWaker]>> {
    queue: SpinMutex<FenceQueue<Arr>>,
    finished: AtomicBool,
}

impl<Arr> Fence<Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Creates a new fence on the empty queue.
    ///
    /// # Safety
    /// The queue *must* be entirely filled as [`MaybeUninit::uninit`].
    pub const unsafe fn new(queue: Arr) -> Self {
        Self {
            queue: SpinMutex::new(unsafe { FenceQueue::new(queue) }),
            finished: AtomicBool::new(false),
        }
    }

    /// Returns if the fence has been released.
    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Returns the underlying capacity.
    pub fn capacity(&self) -> usize {
        self.queue.lock().data.as_mut().len()
    }

    /// Returns the claimed capacity.
    ///
    /// The number of waiters may exceed this if they haven't all claimed an
    /// entry yet.
    pub fn usage(&self) -> usize {
        self.queue.lock().pos
    }
}

impl<Arr> Drop for Fence<Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn drop(&mut self) {
        let mut queue = self.queue.lock();
        let pos = queue.pos;

        // Cleans up any wakers left over by a fence holder
        if pos > 0 {
            // All indicies before the pointer are initialized wakers
            for entry in &mut queue.data.as_mut()[0..pos] {
                let mut local_entry = MaybeUninit::uninit();
                core::mem::swap(entry, &mut local_entry);
                let local_entry = unsafe { local_entry.assume_init() };
                drop(local_entry);
            }
        }
    }
}

impl<Arr> Default for Fence<Arr>
where
    Arr: AsMut<[FenceWaker]> + Default,
{
    fn default() -> Self {
        // Since [`FenceWaker`] is not [`Default`], any [`Default`] container
        // must not initialize its elements. Zero elements meets the all
        // elements uninit criteria.
        unsafe { Self::new(Arr::default()) }
    }
}

/// A [`Fence`] based on a static array.
///
/// Initialize this with [`Self::new_arr`].
pub type StaticFence<const N: usize> = Fence<[FenceWaker; N]>;

#[cfg(feature = "alloc")]
/// A [`Fence`] based on a [`Vec`][`alloc::vec::Vec`].
///
/// Initialize this with [`Self::default`].
pub type VecFence = Fence<alloc::vec::Vec<FenceWaker>>;

impl<const N: usize> StaticFence<N> {
    pub const fn new_arr() -> Self {
        // This works because:
        //  1. All of the elements are still marked MaybeUninit
        //  2. Arrays have no metadata: https://doc.rust-lang.org/reference/type-layout.html#r-layout.array
        let arr = unsafe { MaybeUninit::<[FenceWaker; N]>::uninit().assume_init() };

        // Created with all uninit elements, so this call is safe.
        unsafe { Self::new(arr) }
    }
}

/// Releases the [`Fence`] on [`Drop`].
///
/// This is constructed via [`Fence::hold`].
/// All fields are borrowed from the originating [`Fence`].
/// ANY copy of this for a given [`Fence`] will release the fence.
/// Once the [`Fence`] is released, it cannot be re-enabled.
#[derive(Debug, Clone)]
pub struct FenceHolder<'a, Arr: AsMut<[FenceWaker]>> {
    source: &'a Fence<Arr>,
}

impl<Arr> Fence<Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Produces a handle to release the fence on drop.
    pub fn hold<'a>(&'a self) -> FenceHolder<'a, Arr>
    where
        Arr: 'a,
    {
        FenceHolder { source: self }
    }
}

impl<Arr> FenceHolder<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    pub const fn source(&self) -> &Fence<Arr> {
        self.source
    }
}

impl<Arr> PartialEq for FenceHolder<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn eq(&self, other: &Self) -> bool {
        // The finished comparison can be skipped, as
        core::ptr::eq(self.source as *const _, other.source as *const _)
    }
}

impl<Arr> Eq for FenceHolder<'_, Arr> where Arr: AsMut<[FenceWaker]> {}

impl<Arr> PartialOrd for FenceHolder<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<Arr> Ord for FenceHolder<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        (self.source as *const _ as usize).cmp(&(other.source as *const _ as usize))
    }
}

impl<Arr> Hash for FenceHolder<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        state.write_usize(self.source as *const _ as usize);
    }
}

impl<Arr> FenceHolder<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Allows for calling [`Drop`] without destroying the holder.
    pub fn release(&self) {
        self.source.finished.store(true, Ordering::Release);

        // Cleans up any wakers left over by a fence holder
        let mut queue = self.source.queue.lock();
        if queue.pos > 0 {
            // Zeroing out the queue pointer makes any future FenceHolders skip
            let pos = core::mem::replace(&mut queue.pos, 0);

            // All indicies before the pointer are initialized wakers
            for entry in &mut queue.data.as_mut()[0..pos] {
                let mut local_entry = MaybeUninit::uninit();
                core::mem::swap(entry, &mut local_entry);
                let local_entry = unsafe { local_entry.assume_init() };
                local_entry.wake();
            }
        }
    }
}

impl<Arr> Drop for FenceHolder<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn drop(&mut self) {
        self.release();
    }
}

/// Waits for a [`Fence`] to release via [`FenceHolder`].
///
/// When [`Future::poll`] finishes, the [`Fence`] is released.
///
/// It is possible to produce more waiters than there is capacity in the
/// [`Fence`]. Any excess waiters will check for completion and immediately
/// re-queue instead of efficiently waiting on a callback. Excess waiters are
/// inefficient (they constantly adjust an atomic pointer) and strongly not
/// recommended.
///
/// [`Clone`] creates a new waiter with its own [`Waker`] storage.
#[derive(Debug)]
pub struct FenceWaiter<'a, Arr: AsMut<[FenceWaker]>> {
    state: FenceWaiterState,
    source: &'a Fence<Arr>,
}

impl<Arr> FenceWaiter<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    pub const fn source(&self) -> &Fence<Arr> {
        self.source
    }
}

impl<Arr> PartialEq for FenceWaiter<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn eq(&self, other: &Self) -> bool {
        // The finished comparison can be skipped, as
        (core::ptr::eq(self.source as *const _, other.source as *const _))
            && (self.state == other.state)
    }
}

impl<Arr> Eq for FenceWaiter<'_, Arr> where Arr: AsMut<[FenceWaker]> {}

impl<Arr> PartialOrd for FenceWaiter<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<Arr> Ord for FenceWaiter<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        (self.source as *const _ as usize)
            .cmp(&(other.source as *const _ as usize))
            .then(self.state.cmp(&other.state))
    }
}

impl<Arr> Hash for FenceWaiter<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        state.write_usize(self.source as *const _ as usize);
        self.state.hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum FenceWaiterState {
    Uninitialized,
    Waiting { queue_pos: usize },
}

impl<Arr> Clone for FenceWaiter<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn clone(&self) -> Self {
        Self {
            state: FenceWaiterState::Uninitialized,
            source: self.source,
        }
    }
}

impl<Arr> Fence<Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Produces a handle to wait for the fence to drop.
    pub fn wait<'a>(&'a self) -> FenceWaiter<'a, Arr>
    where
        Arr: 'a,
    {
        FenceWaiter {
            state: FenceWaiterState::Uninitialized,
            source: self,
        }
    }
}

/// Generic function that can be reused by both types of waiters
fn fence_wait<Arr, F>(
    this: &mut Pin<&mut FenceWaiter<Arr>>,
    cx: &mut Context<'_>,
    insert_waiter: F,
) -> Poll<()>
where
    Arr: AsMut<[FenceWaker]>,
    F: FnOnce(&mut FenceWaiterState, &mut FenceQueue<Arr>, &mut Context<'_>),
{
    if this.source.finished.load(Ordering::Acquire) {
        return Poll::Ready(());
    }

    // If the try lock fails (aside from spurious failure), there are two
    // likely scenarios:
    // 1. The holder updated finished and is waking queues
    // 2. Another waiter is inserting into the queue
    //
    // In either case, tossing this to the back of the async queue avoids
    // busy-waiting and may end earlier.
    if let Some(mut queue) = this.source.queue.try_lock_weak() {
        match this.state {
            FenceWaiterState::Uninitialized => {
                // It is possible that finished was updated before the lock
                // was claimed, and this waker would never be notified of that.
                if this.source.finished.load(Ordering::Acquire) {
                    return Poll::Ready(());
                }

                insert_waiter(&mut this.state, &mut queue, cx);
            }
            FenceWaiterState::Waiting { queue_pos } => {
                let waker = &mut queue.data.as_mut()[queue_pos];
                // This must have been in the uninitialized state earlier,
                // and then initialized its waker entry.
                let waker = unsafe { waker.assume_init_mut() };
                waker.clone_from(cx.waker());
            }
        }
        Poll::Pending
    } else {
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl<Arr> Future for FenceWaiter<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        fence_wait(&mut self, cx, |state, queue, fn_cx| {
            let FenceQueue { data, pos } = &mut *queue;
            // Never fills the last element of a usize::MAX array.
            // That is the cost of using a usize::MAX array.
            if *pos < data.as_mut().len() {
                data.as_mut()[*pos] = MaybeUninit::new(fn_cx.waker().clone());
                *state = FenceWaiterState::Waiting { queue_pos: *pos };
                *pos += 1;
            } else {
                fn_cx.waker().wake_by_ref()
            }
        })
    }
}

#[cfg(feature = "alloc")]
/// [`FenceWaiter`] that extends the underlying collection.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct FenceWaiterExtending<'a, Arr: WakerArrExtending>(FenceWaiter<'a, Arr>);

#[cfg(feature = "alloc")]
impl<Arr> Clone for FenceWaiterExtending<'_, Arr>
where
    Arr: WakerArrExtending,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(feature = "alloc")]
impl<'a, Arr> Deref for FenceWaiterExtending<'a, Arr>
where
    Arr: WakerArrExtending,
{
    type Target = FenceWaiter<'a, Arr>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(feature = "alloc")]
impl<Arr> DerefMut for FenceWaiterExtending<'_, Arr>
where
    Arr: WakerArrExtending,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(feature = "alloc")]
impl<'a, Arr> From<FenceWaiterExtending<'a, Arr>> for FenceWaiter<'a, Arr>
where
    Arr: WakerArrExtending,
{
    fn from(value: FenceWaiterExtending<'a, Arr>) -> Self {
        value.0
    }
}

#[cfg(feature = "alloc")]
impl<'a, Arr> From<FenceWaiter<'a, Arr>> for FenceWaiterExtending<'a, Arr>
where
    Arr: WakerArrExtending,
{
    fn from(value: FenceWaiter<'a, Arr>) -> Self {
        FenceWaiterExtending(value)
    }
}

#[cfg(feature = "alloc")]
impl<Arr> Fence<Arr>
where
    Arr: WakerArrExtending,
{
    /// Executes [`WakerArrExtending::reserve`] on the waker array.
    pub fn reserve_storage(&self, additional: usize) {
        self.queue.lock().data.reserve(additional);
    }

    /// Executes [`WakerArrExtending::fill`] on the waker array.
    ///
    /// Can be used to ensure space before converting [`FenceWaiterExtending`]
    /// to [`FenceWaiter`]. Both static and dynamically created waiters can
    /// then be treated as a single type.
    ///
    /// # Example
    /// ```
    /// use async_fence::{Fence, FenceWaker};
    ///
    /// use std::{sync::LazyLock, vec::Vec};
    ///
    /// // This fence has dynamic storage and will live for the entire program.
    /// static FENCE: LazyLock<Fence<Vec<FenceWaker>>> = LazyLock::new(Fence::default);
    ///
    /// let holder = FENCE.hold();
    ///
    /// // This preallocates for 3 non-dynamic wait entries.
    /// FENCE.fill_storage(3);
    ///
    /// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// rt.block_on(async {
    ///
    ///     // The fence storage will not extend further, as this is a regular
    ///     // wait handle.
    ///     let handles: [_; 3] = core::array::from_fn(|_| tokio::spawn(FENCE.wait()));
    ///
    ///     // After the holder is dropped, all the waiters finish.
    ///     drop(holder);
    ///     for handle in handles {
    ///         handle.await;
    ///     }
    /// });
    /// ```
    pub fn fill_storage(&self, additional: usize) {
        self.queue.lock().data.fill(additional);
    }

    /// Produces a handle to wait for the fence to drop.
    ///
    /// Extends the underlying storage when out of space for wakers.
    ///
    /// # Example
    /// ```
    /// use async_fence::{Fence, FenceWaker, VecFence};
    ///
    /// use std::{sync::LazyLock, vec::Vec};
    ///
    /// // This fence has dynamic storage and will live for the entire program.
    /// static FENCE: LazyLock<VecFence> = LazyLock::new(Fence::default);
    ///
    /// // This is an alternative that executes const, and plays by the unsafe
    /// // rules. Const function cannot currently be set for a trait, so this
    /// // has to be done manually.
    /// static UNSAFE_FENCE: VecFence = unsafe { Fence::new(Vec::new()) };
    ///
    /// let holder = FENCE.hold();
    ///
    /// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// rt.block_on(async {
    ///     // The fence storage will extend to fit all the handles.
    ///     let handles: [_; 3] = core::array::from_fn(|_| tokio::spawn(FENCE.wait_extending()));
    ///
    ///     // After the holder is dropped, all the waiters finish.
    ///     drop(holder);
    ///     for handle in handles {
    ///         handle.await;
    ///     }
    /// });
    /// ```
    pub fn wait_extending<'a>(&'a self) -> FenceWaiterExtending<'a, Arr>
    where
        Arr: 'a,
    {
        self.wait().into()
    }
}

#[cfg(feature = "alloc")]
impl<Arr> Future for FenceWaiterExtending<'_, Arr>
where
    Arr: WakerArrExtending,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Since this is a transparent wrapper, the pin extends.
        let mut inner = unsafe { self.map_unchecked_mut(|this| &mut this.0) };
        fence_wait(
            &mut inner,
            cx,
            |state, queue: &mut FenceQueue<Arr>, fn_cx| {
                let FenceQueue { data, pos } = &mut *queue;

                if *pos < usize::MAX {
                    data.push(MaybeUninit::new(fn_cx.waker().clone()));
                    *state = FenceWaiterState::Waiting { queue_pos: *pos };
                    *pos += 1;
                } else {
                    fn_cx.waker().wake_by_ref()
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use futures::poll;
    use tokio::{
        task::JoinSet,
        time::{sleep, timeout},
    };

    use super::*;

    const FENCE_LEN: usize = 3;

    #[tokio::test]
    async fn waits_on_holder() {
        static FENCE: StaticFence<FENCE_LEN> = StaticFence::new_arr();

        let holder = FENCE.hold();

        let mut handles = JoinSet::new();
        for _ in 0..FENCE_LEN {
            handles.spawn(FENCE.wait());
        }

        // None of the waiters finish before the holder is dropped.
        assert!(
            timeout(Duration::from_secs(1), handles.join_next())
                .await
                .is_err()
        );

        // After the holder is dropped, all the waiters finish.
        drop(holder);
        assert!(
            timeout(Duration::from_secs(1), handles.join_all())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn wait_state_transitions() {
        static FENCE: StaticFence<FENCE_LEN> = StaticFence::new_arr();

        let handles: [_; FENCE_LEN] = core::array::from_fn(|_| FENCE.wait());

        for (idx, mut handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.state, FenceWaiterState::Uninitialized);

            // Need to loop because of spurious failures
            let mut state = handle.state;
            for _ in 0..100 {
                assert_eq!(poll!(&mut handle), Poll::Pending);
                state = handle.state;
                if state == (FenceWaiterState::Waiting { queue_pos: idx }) {
                    break;
                }
                sleep(Duration::from_secs(1)).await;
            }
            assert_eq!(state, FenceWaiterState::Waiting { queue_pos: idx });
        }
    }

    #[tokio::test]
    async fn instant_pass_post_hold() {
        static FENCE: StaticFence<FENCE_LEN> = StaticFence::new_arr();

        drop(FENCE.hold());
        let handles: [_; FENCE_LEN] = core::array::from_fn(|_| FENCE.wait());

        for handle in handles {
            assert_eq!(poll!(handle), Poll::Ready(()));
        }
    }

    #[tokio::test]
    async fn multi_fence_hold_drop() {
        static FENCE: StaticFence<FENCE_LEN> = StaticFence::new_arr();

        drop(FENCE.hold());
        let handles: [_; FENCE_LEN] = core::array::from_fn(|_| FENCE.wait());
        // This should have absolutely zero effect on state.
        drop(FENCE.hold());

        for handle in handles {
            assert_eq!(poll!(handle), Poll::Ready(()));
        }
    }

    #[tokio::test]
    async fn excess_waiters() {
        static FENCE: StaticFence<FENCE_LEN> = StaticFence::new_arr();

        let holder = FENCE.hold();

        let mut handles: [_; FENCE_LEN] = core::array::from_fn(|_| FENCE.wait());
        let mut excess_handles: [_; FENCE_LEN * 10] = core::array::from_fn(|_| FENCE.wait());

        // All handles enter Waiting
        for (idx, mut handle) in handles.iter_mut().enumerate() {
            assert_eq!(handle.state, FenceWaiterState::Uninitialized);

            // Need to loop because of spurious failures
            let mut state = handle.state;
            for _ in 0..100 {
                assert_eq!(poll!(&mut handle), Poll::Pending);
                state = handle.state;
                if state == (FenceWaiterState::Waiting { queue_pos: idx }) {
                    break;
                }
                sleep(Duration::from_secs(1)).await;
            }
            assert_eq!(state, FenceWaiterState::Waiting { queue_pos: idx });
        }

        // All excess handles stay in uninitialized
        for mut handle in &mut excess_handles {
            assert_eq!(handle.state, FenceWaiterState::Uninitialized);

            // Need to loop because of spurious failures
            for _ in 0..10 {
                assert_eq!(poll!(&mut handle), Poll::Pending);
                assert_eq!(handle.state, FenceWaiterState::Uninitialized);
            }
        }

        // After the holder is dropped, ALL the waiters finish.
        drop(holder);

        let mut all_handles = JoinSet::new();
        for handle in handles.into_iter().chain(excess_handles) {
            all_handles.spawn(handle);
        }

        assert!(
            timeout(Duration::from_secs(1), all_handles.join_all())
                .await
                .is_ok()
        );
    }

    #[cfg(feature = "alloc")]
    #[tokio::test]
    async fn dynamically_extends() {
        use alloc::vec::Vec;

        let fence: Fence<Vec<_>> = Fence::default();

        let handles: [_; FENCE_LEN] = core::array::from_fn(|_| fence.wait_extending());

        for (idx, mut handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.state, FenceWaiterState::Uninitialized);

            // Need to loop because of spurious failures
            let mut state = handle.state;
            for _ in 0..100 {
                assert_eq!(poll!(&mut handle), Poll::Pending);
                state = handle.state;
                if state == (FenceWaiterState::Waiting { queue_pos: idx }) {
                    break;
                }
                sleep(Duration::from_secs(1)).await;
            }
            assert_eq!(state, FenceWaiterState::Waiting { queue_pos: idx });
        }
    }

    #[cfg(feature = "alloc")]
    #[tokio::test]
    async fn extends_on_fill() {
        use alloc::vec::Vec;

        let fence: Fence<Vec<_>> = Fence::default();

        let mut handles: [_; FENCE_LEN] = core::array::from_fn(|_| fence.wait());

        // All non-reserved handles stay in uninitialized
        for mut handle in &mut handles {
            assert_eq!(handle.state, FenceWaiterState::Uninitialized);

            // Need to loop because of spurious failures
            for _ in 0..10 {
                assert_eq!(poll!(&mut handle), Poll::Pending);
                assert_eq!(handle.state, FenceWaiterState::Uninitialized);
            }
        }

        assert_eq!(fence.usage(), 0);
        assert_eq!(fence.capacity(), 0);

        fence.fill_storage(3);
        assert_eq!(fence.usage(), 0);
        assert_eq!(fence.capacity(), 3);

        for (idx, mut handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.state, FenceWaiterState::Uninitialized);

            // Need to loop because of spurious failures
            let mut state = handle.state;
            for _ in 0..100 {
                assert_eq!(poll!(&mut handle), Poll::Pending);
                state = handle.state;
                if state == (FenceWaiterState::Waiting { queue_pos: idx }) {
                    break;
                }
                sleep(Duration::from_secs(1)).await;
            }
            assert_eq!(state, FenceWaiterState::Waiting { queue_pos: idx });
        }
    }
}
