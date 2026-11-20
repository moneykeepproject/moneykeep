//! Phone capture end-to-end tests (in `tests/`): the desktop's `capture.*` commands,
//! through `mk-connect`'s fetcher, against the real capture server on 127.0.0.1.
//!
//! They live in their own crate, outside the crates other Keep apps copy
//! (crates-for-other-apps.md), so that `mk-connect` has no dependency of any kind on
//! `moneykeep-server`. This library is empty.
