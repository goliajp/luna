//! A run of Lua that must not panic: a panic inside a native becomes a
//! Lua error, which the script's `pcall` may swallow, so the collector's
//! checks (`gc-verify`) and debug assertions that fire there would go
//! unseen otherwise.

use luna_core::__private::{panics_seen, watch_panics};

/// `f()`, failing when this thread panicked meanwhile.
pub(crate) fn panic_free<R>(what: &str, f: impl FnOnce() -> R) -> R {
    watch_panics();
    let before = panics_seen();
    let r = f();
    let n = panics_seen() - before;
    assert_eq!(n, 0, "{what}: {n} panics in the run");
    r
}
