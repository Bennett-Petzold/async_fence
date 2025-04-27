/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#![no_std]

/*!
`async_fence` provies `no_std` async fences and OnceLock.

# Feature Flags
* `alloc`: Enables dynamic queue types via alloc.
*
# Licensing and Contributing

All code is licensed under MPL 2.0. See the [FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)
for license questions. The license is non-viral copyleft and does not block this library from
being used in closed-source codebases. If you are using this library for a commercial purpose,
consider reaching out to `dansecob.dev@gmail.com` to make a financial contribution.

Contributions are welcome at
<https://github.com/Bennett-Petzold/async_fence>.
*/

pub mod core;
#[cfg(feature = "alloc")]
pub use core::VecFence;
pub use core::{Fence, StaticFence};

#[cfg(feature = "alloc")]
pub mod extending_arr;

#[cfg(feature = "once")]
pub mod once;
#[cfg(all(feature = "once", feature = "alloc"))]
pub use once::VecOnceLock;
#[cfg(feature = "once")]
pub use once::{OnceLock, StaticOnceLock};

/*
#[cfg(feature = "lazy")]
pub mod lazy;
#[cfg(feature = "lazy")]
pub use lazy::LazyLock;
#[cfg(all(feature = "lazy", feature = "alloc"))]
pub use lazy::{DynLazyLock, VecLazyLock};
*/
