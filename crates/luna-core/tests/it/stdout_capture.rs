//! What luna writes to standard output, caught per thread: a test runs Lua
//! with the real `print` and `io.write`, which allocate nothing a buffer
//! in Lua would (a script that reads `collectgarbage("count")` sees what it
//! would see under PUC's `lua`).

use std::cell::RefCell;
use std::io::Write;
use std::sync::Once;

thread_local! {
    static CAUGHT: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

fn write(bytes: &[u8]) -> bool {
    CAUGHT.with(|c| match &mut *c.borrow_mut() {
        Some(buf) => {
            buf.extend_from_slice(bytes);
            true
        }
        None => std::io::stdout().write_all(bytes).is_ok(),
    })
}

/// `f`'s result and what it wrote to standard output on this thread.
pub(super) fn caught<R>(f: impl FnOnce() -> R) -> (R, Vec<u8>) {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        luna_core::__private::use_host_stdout(luna_core::__private::HostStdout {
            write,
            flush: || true,
            setvbuf: |_| {},
        })
    });
    CAUGHT.with(|c| *c.borrow_mut() = Some(Vec::new()));
    let r = f();
    let out = CAUGHT.with(|c| c.borrow_mut().take()).unwrap_or_default();
    (r, out)
}
