//! The panics a run of Lua had. A panic inside a native becomes a Lua
//! error, which a script's `pcall` may then swallow, so a check that fires
//! there (a collector check of `gc-verify`, a debug assertion) would go
//! unseen: a test harness runs Lua through [`panics_in`] or
//! [`panic_free`] and fails on any.

use std::cell::Cell;
use std::sync::Once;

thread_local! {
    static SEEN: Cell<u64> = const { Cell::new(0) };
}

static HOOK: Once = Once::new();

/// `f()`, and how many times the thread panicked while it ran. The panic
/// hook set before still runs (the message is printed as usual).
pub fn panics_in<R>(f: impl FnOnce() -> R) -> (R, u64) {
    HOOK.call_once(|| {
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            SEEN.with(|c| c.set(c.get() + 1));
            before(info);
        }));
    });
    let from = SEEN.with(Cell::get);
    let r = f();
    (r, SEEN.with(Cell::get) - from)
}

/// `f()`, failing (`what` names the run) when the thread panicked while it
/// ran.
#[track_caller]
pub fn panic_free<R>(what: &str, f: impl FnOnce() -> R) -> R {
    let (r, n) = panics_in(f);
    assert_eq!(n, 0, "{what}: {n} panics in the run");
    r
}
