use core::{mem::MaybeUninit, ops::RangeBounds};

extern crate alloc;
use alloc::vec::Vec;

use crate::core::FenceWaker;

/// Array wrapper that may extend (e.g. [`Vec`]).
pub trait WakerArrExtending: Default + AsMut<[FenceWaker]> {
    type DrainIter<'a>: IntoIterator<Item = FenceWaker>
    where
        Self: 'a;

    /// Same as [`alloc::vec::Vec::push`].
    fn push(&mut self, value: FenceWaker);
    /// Same as [`alloc::vec::Vec::reserve`].
    fn reserve(&mut self, additional: usize);
    /// Same as [`alloc::vec::Vec::drain`].
    ///
    /// Held for future optimizations with generic specialization.
    fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Self::DrainIter<'_>;

    /// Creates `additional` uninitialized entries.
    fn fill(&mut self, additional: usize) {
        self.reserve(additional);
        for _ in 0..additional {
            self.push(MaybeUninit::uninit());
        }
    }
}

impl WakerArrExtending for Vec<FenceWaker> {
    type DrainIter<'a>
        = alloc::vec::Drain<'a, FenceWaker>
    where
        Self: 'a;

    fn push(&mut self, value: FenceWaker) {
        Vec::push(self, value)
    }
    fn reserve(&mut self, additional: usize) {
        Vec::reserve(self, additional);
    }
    fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Self::DrainIter<'_> {
        Vec::drain(self, range)
    }

    fn fill(&mut self, additional: usize) {
        self.extend(core::iter::repeat_with(MaybeUninit::uninit).take(additional));
    }
}
