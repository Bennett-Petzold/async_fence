use core::future::Ready;

use crate::{OnceLock, StaticOnceLock, core::FenceWaker, once::OnceLockInit};

#[cfg(feature = "alloc")]
extern crate alloc;

/// Async equivalent of
/// [`std::sync::LazyLock`](<https://doc.rust-lang.org/stable/std/sync/struct.LazyLock.html>).
///
/// Construct with [`Self::new`]. Resolve with [`Self::get`].
/// Use [`DynLazyLock`] for dynamic backing.
///
/// Produces futures that either set with the given future, or wait for another
/// future to set the value. The methods never block. Waiting futures will,
/// however, not pick up initialization when it is dropped.
///
/// # Example
/// ```
/// use async_fence::LazyLock;
///
/// use core::future::ready;
///
/// const FENCE_LEN: usize = 1;
///
/// static LOCK: LazyLock<bool, FENCE_LEN> = LazyLock::new(|| ready(true));
///
/// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// rt.block_on(async {
///     let (fut_0, fut_1) = (LOCK.get(), LOCK.get());
///     // Both futures return the single initialized value.
///     assert_eq!(fut_0.await, fut_1.await);
/// });
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct LazyLock<T, const N: usize, F = fn() -> Ready<T>> {
    lock: StaticOnceLock<T, N>,
    init: F,
}

impl<T, Fut, const N: usize, F> LazyLock<T, N, F>
where
    Fut: Future<Output = T>,
    F: Fn() -> Fut,
{
    /// Creates a new lazy value with the given initializing function.
    pub const fn new(init: F) -> Self {
        Self {
            lock: OnceLock::new_arr(),
            init,
        }
    }

    /// Resolves the lock into the evaluated value.
    pub fn get(&self) -> OnceLockInit<'_, T, [FenceWaker; N], Fut> {
        self.lock.get_or_init((self.init)())
    }
}
