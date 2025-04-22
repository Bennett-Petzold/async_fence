use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering, fence},
};

use crate::{Fence, FenceWaker};

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

/// Standard removal of the !Sync forced by [`UnsafeCell`].
unsafe impl<T, Arr: AsMut<[FenceWaker]>> Sync for OnceLock<T, Arr> where T: Send + Sync {}

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

    pub fn into_inner(self) -> Option<T> {
        if self.fence.finished() {
            // Memory ordering assures the single write to data is complete
            fence(Ordering::Acquire);

            // Finished indicates initialized
            Some(unsafe { self.data.into_inner().assume_init() })
        } else {
            None
        }
    }

    pub const fn get_mut_or_init<F>(&mut self, fut: F) -> Option<&mut T>
    where
        F: Future<Output = T>,
    {
        todo!()
    }

    pub const fn get_mut_or_try_init<F, E>(&mut self, fut: F) -> Option<&mut T>
    where
        F: Future<Output = Result<T, E>>,
    {
        todo!()
    }

    pub const fn get_or_init<F>(&self, fut: F) -> Option<&T>
    where
        F: Future<Output = T>,
    {
        todo!()
    }

    pub const fn get_or_try_init<F, E>(&self, fut: F) -> Option<&T>
    where
        F: Future<Output = Result<T, E>>,
    {
        todo!()
    }

    pub fn set(&self, value: T) -> Result<(), T> {
        let existing_holder = self.holder_exists.swap(true, Ordering::AcqRel);

        if !existing_holder {
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
        let existing_holder = self.holder_exists.swap(true, Ordering::AcqRel);

        if !existing_holder {
            // existing_holder guards so that this is the only access.
            *unsafe { &mut *self.data.get() } = MaybeUninit::new(value);

            // Memory fencing ensures other threads see the data change
            fence(Ordering::Release);
            drop(self.fence.hold());

            // Guaranteed by prior operations
            Ok(unsafe { (&*self.data.get()).assume_init_ref() })
        } else {
            // Memory fencing ensures this got the initialized data change
            fence(Ordering::Acquire);
            let exisiting = unsafe { (&*self.data.get()).assume_init_ref() };

            Err((exisiting, value))
        }
    }

    pub const fn wait<F>(&self, value: T) -> Option<&T> {
        todo!()
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
