/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

/*! Defines the core async wait for a waker. */

use core::{
    mem::MaybeUninit,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};

use spin::mutex::SpinMutex;

#[cfg(feature = "alloc")]
extern crate alloc;

pub type FenceWaker = MaybeUninit<Waker>;

#[derive(Debug)]
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
    queue: &'a SpinMutex<FenceQueue<Arr>>,
    finished: &'a AtomicBool,
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
        FenceHolder {
            queue: &self.queue,
            finished: &self.finished,
        }
    }
}

impl<Arr> Drop for FenceHolder<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);

        // Cleans up any wakers left over by a fence holder
        let mut queue = self.queue.lock();
        if queue.pos > 0 {
            // Zeroing out the queue pointer makes any future FenceHolders skip
            let pos = core::mem::replace(&mut queue.pos, 0);

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
    queue: &'a SpinMutex<FenceQueue<Arr>>,
    finished: &'a AtomicBool,
}

#[derive(Debug)]
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
            queue: self.queue,
            finished: self.finished,
        }
    }
}

impl<Arr> Fence<Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Produces a handle to release the fence on drop.
    pub fn wait<'a>(&'a self) -> FenceWaiter<'a, Arr>
    where
        Arr: 'a,
    {
        FenceWaiter {
            state: FenceWaiterState::Uninitialized,
            queue: &self.queue,
            finished: &self.finished,
        }
    }
}

impl<Arr> Future for FenceWaiter<'_, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.finished.load(Ordering::Acquire) {
            return Poll::Ready(());
        }

        // If the try lock fails (aside from spurious failure), there are two
        // likely scenarios:
        // 1. The holder updated finished and is waking queues
        // 2. Another waiter is inserting into the queue
        //
        // In either case, tossing this to the back of the async queue avoids
        // busy-waiting and may end earlier.
        if let Some(mut queue) = self.queue.try_lock_weak() {
            match self.state {
                FenceWaiterState::Uninitialized => {
                    // It is possible that finished was updated before the lock
                    // was claimed, and this waker would never be notified of that.
                    if self.finished.load(Ordering::Acquire) {
                        return Poll::Ready(());
                    }

                    let FenceQueue { data, pos } = &mut *queue;
                    let data = data.as_mut();

                    // Never fills the last element of a usize::MAX array.
                    // That is the cost of using a usize::MAX array.
                    if *pos < data.len() {
                        data[*pos] = MaybeUninit::new(cx.waker().clone());
                        self.state = FenceWaiterState::Waiting { queue_pos: *pos };
                        *pos += 1;
                    } else {
                        cx.waker().wake_by_ref();
                    }
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
}
