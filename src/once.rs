use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    panic::RefUnwindSafe,
    pin::{Pin, pin},
    sync::atomic::{AtomicBool, Ordering, fence},
    task::{Context, Poll, ready},
};

use pin_project_lite::pin_project;

use crate::core::{Fence, FenceWaiter, FenceWaker};

#[cfg(feature = "alloc")]
extern crate alloc;

/// Async equivalent of
/// [`std::sync::OnceLock`](<https://doc.rust-lang.org/stable/std/sync/struct.OnceLock.html>).
///
/// Construct with [`Self::new_arr`] for static backings.
/// Use [`Self::default`] for dynamic backings.
///
/// Produces futures that either set or wait for another future to set the
/// value. The methods never block. [`Self::get_or_init`] is the main method.
///
/// # Example
/// ```
/// use async_fence::once::StaticOnceLock;
///
/// const FENCE_LEN: usize = 1;
///
/// static LOCK: StaticOnceLock<bool, FENCE_LEN> = StaticOnceLock::new_arr();
///
/// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// rt.block_on(async {
///     let regular = tokio::spawn(LOCK.get_or_init(async { true }));
///     let opposite = tokio::spawn(LOCK.get_or_init(async { false }));
///
///     // Only one of the initializers runs, and the other shares its value.
///     assert_eq!(regular.await.unwrap(), opposite.await.unwrap());
/// });
/// ```
#[derive(Debug)]
pub struct OnceLock<T, Arr: AsMut<[FenceWaker]>> {
    data: UnsafeCell<MaybeUninit<T>>,
    // When this is finished, `data` is initialized.
    // If data is uninitialized, this must be reset.
    fence: Fence<Arr>,
    // Only the call that claims this boolean can set `data`.
    holder_exists: AtomicBool,
}

/// Standard removal of the !Send forced by [`UnsafeCell`].
unsafe impl<T, Arr: AsMut<[FenceWaker]>> Send for OnceLock<T, Arr> where T: Send {}
/// Standard removal of the !Sync forced by [`UnsafeCell`].
unsafe impl<T, Arr: AsMut<[FenceWaker]>> Sync for OnceLock<T, Arr> where T: Send + Sync {}

impl<T, Arr: AsMut<[FenceWaker]>> RefUnwindSafe for OnceLock<T, Arr> {}

impl<T, Arr: AsMut<[FenceWaker]>> Drop for OnceLock<T, Arr> {
    fn drop(&mut self) {
        if self.holder_exists.load(Ordering::Acquire) && self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);

            let mut data = MaybeUninit::uninit();
            core::mem::swap(self.data.get_mut(), &mut data);

            // Finished indicates initialized
            let data = unsafe { data.assume_init() };
            // Make sure initialized data has destructor calls
            drop(data);
        }
    }
}

impl<T: PartialEq, Arr: AsMut<[FenceWaker]>> PartialEq for OnceLock<T, Arr> {
    fn eq(&self, other: &Self) -> bool {
        self.get() == other.get()
    }
}

impl<T: Eq, Arr: AsMut<[FenceWaker]>> Eq for OnceLock<T, Arr> {}

impl<T, Arr> OnceLock<T, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Creates a new [`Self`] backed by the empty queue.
    ///
    /// # Safety
    /// The queue *must* be entirely filled as [`MaybeUninit::uninit`].
    pub const unsafe fn new(queue: Arr) -> Self {
        Self {
            data: UnsafeCell::new(MaybeUninit::uninit()),
            fence: unsafe { Fence::new(queue) },
            holder_exists: AtomicBool::new(false),
        }
    }
}

/// A [`OnceLock`] backed by a static array.
///
/// Initialize this with [`Self::new_arr`].
pub type StaticOnceLock<T, const N: usize> = OnceLock<T, [FenceWaker; N]>;

#[cfg(feature = "alloc")]
/// A [`OnceLock`] backed by a [`Vec`][`alloc::vec::Vec`].
///
/// Initialize this with [`Self::default`].
pub type VecOnceLock<T> = OnceLock<T, alloc::vec::Vec<FenceWaker>>;

impl<T, const N: usize> StaticOnceLock<T, N> {
    pub const fn new_arr() -> Self {
        Self {
            data: UnsafeCell::new(MaybeUninit::uninit()),
            fence: Fence::new_arr(),
            holder_exists: AtomicBool::new(false),
        }
    }
}

impl<T, Arr> Default for OnceLock<T, Arr>
where
    Arr: AsMut<[FenceWaker]> + Default,
{
    fn default() -> Self {
        Self {
            data: UnsafeCell::new(MaybeUninit::uninit()),
            fence: Fence::default(),
            holder_exists: AtomicBool::new(false),
        }
    }
}

// ---------- Sync functions ---------- //
impl<T, Arr> OnceLock<T, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Returns a reference to the underlying value if initialized.
    pub fn get(&self) -> Option<&T> {
        if self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);
            let data = unsafe { &*self.data.get() };

            // Finished indicates initialized
            Some(unsafe { data.assume_init_ref() })
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying value if initialized.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        if self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);

            // Finished indicates initialized
            Some(unsafe { self.data.get_mut().assume_init_mut() })
        } else {
            None
        }
    }

    /// Returns the underlying value if initialized.
    pub fn into_inner(mut self) -> Option<T> {
        if self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);

            // This type has Drop, so the data needs to be swapped out
            let mut data = MaybeUninit::uninit();
            core::mem::swap(self.data.get_mut(), &mut data);
            // Avoid calling the destructor on uninitialized data
            self.holder_exists.store(false, Ordering::Release);

            // Finished indicates initialized
            Some(unsafe { data.assume_init() })
        } else {
            None
        }
    }

    /// Internal method for only one call to claim initialization.
    fn prior_holder(&self) -> bool {
        self.holder_exists.swap(true, Ordering::AcqRel)
    }

    /// Immediately sets the internal value if uninitialized.
    ///
    /// On failure, the unset value is returned.
    pub fn set(&self, value: T) -> Result<(), T> {
        if !self.prior_holder() {
            // existing_holder guards so that this is the only access.
            *unsafe { &mut *self.data.get() } = MaybeUninit::new(value);

            // Memory fencing ensures other threads see the data change
            fence(Ordering::Release);
            self.fence.release();
            Ok(())
        } else {
            Err(value)
        }
    }

    /// Tries to immediately set and return the internal value if uninitialized.
    ///
    /// On success, a reference to the set value is returned.
    /// On failure, the already-set value and the unset value are returned
    /// (ALREADY_SET, value).
    pub fn try_insert(&self, value: T) -> Result<&T, (&T, T)> {
        if !self.prior_holder() {
            // existing_holder guards so that this is the only access.
            *unsafe { &mut *self.data.get() } = MaybeUninit::new(value);

            // Memory fencing ensures other threads see the data change
            fence(Ordering::Release);
            self.fence.release();

            // Guaranteed by prior operations
            Ok(unsafe { (*self.data.get()).assume_init_ref() })
        } else {
            // Memory fencing ensures this got the initialized data change
            fence(Ordering::Acquire);
            let exisiting = unsafe { (*self.data.get()).assume_init_ref() };

            Err((exisiting, value))
        }
    }
}

impl<T, const N: usize> StaticOnceLock<T, N> {
    /// Returns the underlying value if initialized, resetting to default.
    ///
    /// Only works on static backing arrays.
    pub fn take_arr(&mut self) -> Option<T> {
        if self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);

            self.fence = Fence::new_arr();
            self.holder_exists.store(false, Ordering::Release);
            let data = core::mem::replace(self.data.get_mut(), MaybeUninit::uninit());
            // Finished indicates initialized
            Some(unsafe { data.assume_init() })
        } else {
            None
        }
    }
}

impl<T, Arr> OnceLock<T, Arr>
where
    Arr: AsMut<[FenceWaker]> + Default,
{
    /// Returns the underlying value if initialized, resetting to default.
    ///
    /// Only works on dynamic backing arrays.
    pub fn take_extending(&mut self) -> Option<T> {
        if self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);

            self.fence = Fence::default();
            self.holder_exists.store(false, Ordering::Release);
            let data = core::mem::replace(self.data.get_mut(), MaybeUninit::uninit());
            // Finished indicates initialized
            Some(unsafe { data.assume_init() })
        } else {
            None
        }
    }
}

// ---------- Async functions ---------- //

/// Finishes with `Some(T)` on [`OnceLock`] initialization.
#[derive(Debug)]
pub struct OnceLockWait<'a, T, Arr: AsMut<[FenceWaker]>> {
    handle: FenceWaiter<'a, Arr>,
    data: &'a UnsafeCell<MaybeUninit<T>>,
}

/// Standard removal of the !Send forced by [`UnsafeCell`].
unsafe impl<T, Arr: AsMut<[FenceWaker]>> Send for OnceLockWait<'_, T, Arr> where T: Send {}
/// Standard removal of the !Sync forced by [`UnsafeCell`].
unsafe impl<T, Arr: AsMut<[FenceWaker]>> Sync for OnceLockWait<'_, T, Arr> where T: Send + Sync {}

impl<'a, T, Arr> Future for OnceLockWait<'a, T, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    type Output = &'a T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Waiter must complete before a result is ready/valid
        ready!(pin!(&mut this.handle).poll(cx));

        // Ordering ensures initialized data is synced
        fence(Ordering::Acquire);
        let data = unsafe { (*this.data.get()).assume_init_ref() };
        Poll::Ready(data)
    }
}

pin_project! {
    /// Initializes [`OnceLock`] with the given future, returning the value.
    ///
    /// If dropped mid-initialization, any following code can claim
    /// initialization. Pure waiters prior to the drop will not attempt to init.
    #[derive(Debug)]
    pub struct OnceLockSet<'a, T, Arr: AsMut<[FenceWaker]>, F: Future<Output = T>> {
        lock: &'a OnceLock<T, Arr>,
        #[pin]
        fut: F,
    }

    impl<'a, T, Arr, F> PinnedDrop for OnceLockSet<'a, T, Arr, F>
    where
        Arr: AsMut<[FenceWaker]>,
        F: Future<Output = T>,
    {
        fn drop(this: Pin<&mut Self>) {
            if !this.lock.fence.finished() {
                this.lock.holder_exists.store(false, Ordering::Release);
            }
        }
    }
}

/// Standard removal of the !Send forced by [`UnsafeCell`].
unsafe impl<T, Arr: AsMut<[FenceWaker]>, F: Future<Output = T>> Send for OnceLockSet<'_, T, Arr, F> where
    T: Send
{
}
/// Standard removal of the !Sync forced by [`UnsafeCell`].
unsafe impl<T, Arr: AsMut<[FenceWaker]>, F: Future<Output = T>> Sync for OnceLockSet<'_, T, Arr, F> where
    T: Send + Sync
{
}

impl<'a, T, Arr, F> Future for OnceLockSet<'a, T, Arr, F>
where
    Arr: AsMut<[FenceWaker]>,
    F: Future<Output = T>,
{
    type Output = &'a T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // Waiter must complete before a result is ready/valid
        let init_data = ready!(this.fut.poll(cx));
        // Guaranteed unique access
        let data = unsafe { &mut *this.lock.data.get() };
        *data = MaybeUninit::new(init_data);

        // Ordering ensures initialized data is synced
        fence(Ordering::Release);
        this.lock.fence.release();

        // Previously initialized
        Poll::Ready(unsafe { data.assume_init_ref() })
    }
}

pin_project! {
    /// Either initializes the [`OnceLock`] or waits for another initializer.
    ///
    /// In either case, this returns the value once it has been initialized.
    ///
    /// If the [`Self::Set`] is dropped before initializing, another
    /// [`Self::Set`] can be created. The existing [`Self::Wait`] will not
    /// claim initialization, they will just wait.
    #[project = OnceLockInitProj]
    #[derive(Debug)]
    pub enum OnceLockInit<'a, T, Arr: AsMut<[FenceWaker]>, F: Future<Output = T>> {
        Set{ #[pin] set: OnceLockSet<'a, T, Arr, F> },
        Wait{ wait: OnceLockWait<'a, T, Arr> },
    }
}

impl<'a, T, Arr, F> Future for OnceLockInit<'a, T, Arr, F>
where
    Arr: AsMut<[FenceWaker]>,
    F: Future<Output = T>,
{
    type Output = &'a T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            OnceLockInitProj::Set { set } => set.poll(cx),
            OnceLockInitProj::Wait { wait } => pin!(wait).poll(cx),
        }
    }
}

impl<T, Arr> OnceLock<T, Arr>
where
    Arr: AsMut<[FenceWaker]>,
{
    /// Returns a future for the inner value.
    ///
    /// If the value is uninitialized, and not currently being initialized,
    /// `fut` will be used to initialize it.
    ///
    /// If the initializer is dropped, the next call will produce a new
    /// initializer. Other initializing methods will also apply. No existing
    /// waiters will switch to initializers, they will just wait.
    pub fn get_or_init<F>(&self, fut: F) -> OnceLockInit<'_, T, Arr, F>
    where
        F: Future<Output = T>,
    {
        if self.prior_holder() {
            OnceLockInit::Wait {
                wait: OnceLockWait {
                    handle: self.fence.wait(),
                    data: &self.data,
                },
            }
        } else {
            OnceLockInit::Set {
                set: OnceLockSet { lock: self, fut },
            }
        }
    }

    /// Waits for initialization and returns that value.
    ///
    /// If the inner value is never initialized, the future will never
    /// complete.
    pub fn wait(&self) -> OnceLockWait<'_, T, Arr> {
        OnceLockWait {
            handle: self.fence.wait(),
            data: &self.data,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::future::ready;

    use futures::poll;

    use super::*;

    const FENCE_LEN: usize = 2;
    type TestLock = StaticOnceLock<bool, FENCE_LEN>;

    #[test]
    fn only_sets_once() {
        let mut lock: TestLock = StaticOnceLock::new_arr();

        assert_eq!(lock.get(), None);
        assert_eq!(lock.get_mut(), None);
        assert_eq!(lock.set(true), Ok(()));
        assert_eq!(lock.get(), Some(&true));
        assert_eq!(lock.get_mut(), Some(&mut true));

        assert_eq!(lock.set(false), Err(false));
        assert_eq!(lock.into_inner(), Some(true));
    }

    #[test]
    fn try_insert_once() {
        static LOCK: TestLock = StaticOnceLock::new_arr();

        assert_eq!(LOCK.try_insert(true), Ok(&true));
        assert_eq!(LOCK.try_insert(false), Err((&true, false)));
        assert_eq!(LOCK.try_insert(true), Err((&true, true)));
    }

    #[test]
    fn resets_arr() {
        let mut lock: TestLock = StaticOnceLock::new_arr();

        assert_eq!(lock.set(true), Ok(()));
        assert_eq!(lock.take_arr(), Some(true));
        assert_eq!(lock.get(), None);
        assert_eq!(lock.set(false), Ok(()));
        assert_eq!(lock.get(), Some(&false));
    }

    #[test]
    #[cfg(feature = "alloc")]
    fn resets_dyn() {
        let mut lock: VecOnceLock<_> = VecOnceLock::default();

        assert_eq!(lock.set(true), Ok(()));
        assert_eq!(lock.take_extending(), Some(true));
        assert_eq!(lock.get(), None);
        assert_eq!(lock.set(false), Ok(()));
        assert_eq!(lock.get(), Some(&false));
    }

    #[tokio::test]
    async fn waits_to_init() {
        static LOCK: TestLock = StaticOnceLock::new_arr();

        let init = LOCK.get_or_init(ready(true));
        assert!(matches!(init, OnceLockInit::Set { .. }));
        assert_eq!(LOCK.get(), None);

        let mut wait_0 = LOCK.get_or_init(ready(false));
        assert!(matches!(wait_0, OnceLockInit::Wait { .. }));
        assert_eq!(poll!(&mut wait_0), Poll::Pending);

        let mut wait_1 = LOCK.wait();
        assert_eq!(poll!(&mut wait_1), Poll::Pending);

        assert_eq!(poll!(init), Poll::Ready(&true));
        assert_eq!(poll!(wait_0), Poll::Ready(&true));
        assert_eq!(poll!(wait_1), Poll::Ready(&true));
    }

    #[tokio::test]
    async fn dropped_init() {
        static LOCK: TestLock = StaticOnceLock::new_arr();

        let init = LOCK.get_or_init(ready(true));
        assert!(matches!(init, OnceLockInit::Set { .. }));
        assert_eq!(LOCK.get(), None);

        let mut wait_0 = LOCK.get_or_init(ready(false));
        assert!(matches!(wait_0, OnceLockInit::Wait { .. }));
        assert_eq!(poll!(&mut wait_0), Poll::Pending);

        let mut wait_1 = LOCK.wait();
        assert_eq!(poll!(&mut wait_1), Poll::Pending);

        // Dropping the initializer prevents waits from finishing.
        drop(init);
        assert_eq!(poll!(&mut wait_0), Poll::Pending);
        assert_eq!(poll!(&mut wait_1), Poll::Pending);

        // A later new initializer is applied instead of the dropped one.
        let new_init = LOCK.get_or_init(ready(false));
        assert!(matches!(new_init, OnceLockInit::Set { .. }));
        assert_eq!(poll!(new_init), Poll::Ready(&false));
        assert_eq!(poll!(wait_0), Poll::Ready(&false));
        assert_eq!(poll!(wait_1), Poll::Ready(&false));
    }
}
