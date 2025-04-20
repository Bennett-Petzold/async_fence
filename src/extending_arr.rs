use core::ops::RangeBounds;

extern crate alloc;
use alloc::vec::Vec;

use crate::FenceWaker;

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
    fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Self::DrainIter<'_>;
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
}
