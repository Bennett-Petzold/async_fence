use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    panic::RefUnwindSafe,
    pin::{Pin, pin},
    sync::atomic::{AtomicBool, Ordering, fence},
    task::{Context, Poll, ready},
};

use pin_project_lite::pin_project;

use crate::{Fence, FenceHolder, FenceWaiter, FenceWaker};

#[cfg(feature = "alloc")]
extern crate alloc;

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

    fn prior_holder(&self) -> bool {
        self.holder_exists.swap(true, Ordering::AcqRel)
    }

    pub fn set(&self, value: T) -> Result<(), T> {
        if !self.prior_holder() {
            // existing_holder guards so that this is the only access.
            *unsafe { &mut *self.data.get() } = MaybeUninit::new(value);

            // Memory fencing ensures other threads see the data change
            fence(Ordering::Release);
            drop(self.fence.hold());
            Ok(())
        } else {
            Err(value)
        }
    }

    pub fn try_insert<F>(&self, value: T) -> Result<&T, (&T, T)> {
        if !self.prior_holder() {
            // existing_holder guards so that this is the only access.
            *unsafe { &mut *self.data.get() } = MaybeUninit::new(value);

            // Memory fencing ensures other threads see the data change
            fence(Ordering::Release);
            drop(self.fence.hold());

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
    pub fn take_arr(&mut self) -> Option<T> {
        if self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);

            self.fence = Fence::new_arr();
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
    pub fn take_extending(&mut self) -> Option<T> {
        if self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);

            self.fence = Fence::default();
            let data = core::mem::replace(self.data.get_mut(), MaybeUninit::uninit());
            // Finished indicates initialized
            Some(unsafe { data.assume_init() })
        } else {
            None
        }
    }
}

// ---------- Async functions ---------- //

#[derive(Debug)]
pub struct OnceLockWait<'a, T, Arr: AsMut<[FenceWaker]>> {
    handle: FenceWaiter<'a, Arr>,
    data: &'a UnsafeCell<MaybeUninit<T>>,
}

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
    #[derive(Debug)]
    pub struct OnceLockSet<'a, T, Arr: AsMut<[FenceWaker]>, F: Future<Output = T>> {
        handle: FenceHolder<'a, Arr>,
        data: &'a UnsafeCell<MaybeUninit<T>>,
        #[pin]
        fut: F,
    }
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
        let data = unsafe { &mut *this.data.get() };
        *data = MaybeUninit::new(init_data);

        // Ordering ensures initialized data is synced
        fence(Ordering::Release);
        this.handle.release();

        // Previously initialized
        Poll::Ready(unsafe { data.assume_init_ref() })
    }
}

pin_project! {
    #[project = OnceLockInitProj]
    #[derive(Debug)]
    pub enum OnceLockInit<'a, T, Arr: AsMut<[FenceWaker]>, F: Future<Output = T>> {
        Set{#[pin] set: OnceLockSet<'a, T, Arr, F>},
        Wait{wait: OnceLockWait<'a, T, Arr>},
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
                set: OnceLockSet {
                    handle: self.fence.hold(),
                    data: &self.data,
                    fut,
                },
            }
        }
    }

    pub fn wait(&self) -> OnceLockWait<'_, T, Arr> {
        OnceLockWait {
            handle: self.fence.wait(),
            data: &self.data,
        }
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct OnceLockTrySet<'a, T, E, Arr: AsMut<[FenceWaker]>, F: Future<Output = Result<T, E>>> {
        handle: FenceHolder<'a, Arr>,
        data: &'a UnsafeCell<MaybeUninit<T>>,
        #[pin]
        fut: F,
    }
}

impl<'a, T, E, Arr, F> Future for OnceLockTrySet<'a, T, E, Arr, F>
where
    Arr: AsMut<[FenceWaker]>,
    F: Future<Output = Result<T, E>>,
{
    type Output = Result<&'a T, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // Waiter must complete before a result is ready/valid
        match ready!(this.fut.poll(cx)) {
            Ok(init_data) => {
                // Guaranteed unique access
                let data = unsafe { &mut *this.data.get() };
                *data = MaybeUninit::new(init_data);

                // Ordering ensures initialized data is synced
                fence(Ordering::Release);
                this.handle.release();

                // Previously initialized
                Poll::Ready(Ok(unsafe { data.assume_init_ref() }))
            }
            Err(init_err) => Poll::Ready(Err(init_err)),
        }
    }
}
