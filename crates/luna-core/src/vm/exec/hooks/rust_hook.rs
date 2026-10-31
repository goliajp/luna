//! The Rust hook's run on an event.

use super::*;

impl Vm {
    /// The Rust hook `rh` on `event`, if it is one Rust hooks see: a
    /// synchronous call, which does not reenter the VM through
    /// `call_value`.
    pub(super) fn run_rust_hook(&mut self, rh: RustDebugHook, event: &[u8], line: Option<i64>) {
        let evt = match event {
            b"call" => RustHookEvent::Call,
            b"return" => RustHookEvent::Return,
            b"tail call" | b"tail return" => RustHookEvent::TailCall,
            b"line" => RustHookEvent::Line(line.unwrap_or(0).max(0) as u32),
            b"count" => RustHookEvent::Count,
            _ => return,
        };
        let was_in_hook = self.in_hook;
        self.in_hook = true;
        // PUC `luaD_hook` roots the whole running frame while a hook runs: a
        // register written after the last safe point may sit above `gc_top`,
        // and the hook may collect
        let gc_top = self.gc_top;
        self.gc_top = gc_top.max(self.stack.len() as u32);
        rh(self, evt);
        self.gc_top = gc_top;
        self.in_hook = was_in_hook;
        self.trap = true;
    }
}
