//! The panics a thread has had. A panic inside a native becomes a Lua
//! error, which a script's `pcall` may then swallow: a test that runs Lua
//! under the collector's checks counts them here and fails on any.

use std::cell::Cell;
use std::sync::Once;

thread_local! {
    static SEEN: Cell<u64> = const { Cell::new(0) };
}

static HOOK: Once = Once::new();

/// From now on, count each panic on the thread it happens on; the hook
/// set before still runs (the message is printed as usual).
pub fn watch_panics() {
    HOOK.call_once(|| {
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            SEEN.with(|c| c.set(c.get() + 1));
            before(info);
        }));
    });
}

/// The panics this thread has had since [`watch_panics`] was first called.
pub fn panics_seen() -> u64 {
    SEEN.with(Cell::get)
}
