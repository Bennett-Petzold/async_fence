[![Crate][CrateStatus]][Crate]
[![Tests][TestsStatus]][Tests]
[![Docs][PagesStatus]][Docs]
[![Coverage][Coverage]][CoveragePages]

# async\_fence
`no_std` async fences and OnceLock.
Defines a core `async_fence` that creates one notifier and some number of
waiters.
The number of waiters can be either static (for `no_alloc`), or dynamic (with
the `alloc` feature).
All other abstractions are built on top of `async_fence` for that same static
and dynamic support.

## Other solutions
* [Tokio's OnceCell][TokioOnceCell]
    * Is alloc because it uses a linked list internally.
* [async-once-cell][AsyncOnceCell]
    * Is alloc because it uses a Vec internally.
* [Embassy's OnceLock][EmbassyOnceLock]
    * Is blocking instead of async.

[CrateStatus]: https://img.shields.io/crates/v/async_fence.svg
[Crate]: https://crates.io/crates/async_fence
[TestsStatus]: https://github.com/Bennett-Petzold/async_fence/actions/workflows/all-tests.yml/badge.svg?branch=main
[Tests]: https://github.com/Bennett-Petzold/async_fence/actions/workflows/all-tests.yml
[PagesStatus]: https://github.com/Bennett-Petzold/async_fence/actions/workflows/pages.yml/badge.svg?branch=main
[Docs]: https://bennett-petzold.github.io/async_fence/docs/async_fence/
[Coverage]: https://bennett-petzold.github.io/async_fence/coverage/badge.svg
[CoveragePages]: https://bennett-petzold.github.io/async_fence/coverage/

[TokioOnceCell]: https://docs.rs/tokio/latest/tokio/sync/struct.OnceCell.html 
[AsyncOnceCell]: https://lib.rs/crates/async-once-cell
[EmbassyOnceLock]: https://docs.embassy.dev/embassy-sync/git/default/once_lock/struct.OnceLock.html
