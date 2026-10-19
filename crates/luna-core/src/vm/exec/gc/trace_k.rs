//! How the accounting trace shows a checkpoint: the line PUC's
//! `lua_getstack(L, 0)` would report there, and where luna made it.

use super::*;

impl Vm {
    /// A checkpoint in the accounting trace: `k` and the running Lua
    /// function's current line, 0 in a native.
    #[cfg(feature = "gc-trace")]
    #[cfg_attr(feature = "gc-trace", track_caller)]
    pub(in crate::vm::exec) fn trace_checkpoint(&mut self) {
        // the current line of level 0 (`lua_getstack(L, 0)`): none for a C
        // function or a function just entered; the levels below are not
        // looked at, which a deep stack would make every checkpoint pay for
        let ts = crate::vm::callstack::ThreadStack::with_levels(
            self.version <= LuaVersion::Lua51,
            &self.frames,
            &self.stack,
            self.top,
            &self.running_natives[self.natives_base..],
            None,
            1,
        );
        let line = match ts.levels.first() {
            _ if self.heap.trace.in_c => 0,
            Some(&crate::vm::callstack::DbgKind::Lua(fi))
                if ts.lua(fi).pc > 0 || self.heap.trace.call_hook =>
            {
                ts.currentline(0).max(0)
            }
            _ => 0,
        };
        self.heap.trace.line(format_args!("k {line}"));
        // where luna made it, for finding the call PUC makes there
        let at = std::panic::Location::caller();
        self.heap
            .trace
            .line(format_args!("kw {}:{}", at.file(), at.line()));
    }

    /// `f`, its checkpoints shown in the accounting trace as made in a C
    /// function (`in_c`) or in a call hook (`call_hook`) of PUC's.
    pub(in crate::vm::exec) fn traced_as<R>(
        &mut self,
        in_c: bool,
        call_hook: bool,
        f: impl FnOnce(&mut Vm) -> R,
    ) -> R {
        #[cfg(feature = "gc-trace")]
        let was = (self.heap.trace.in_c, self.heap.trace.call_hook);
        #[cfg(feature = "gc-trace")]
        {
            (self.heap.trace.in_c, self.heap.trace.call_hook) = (in_c, call_hook);
        }
        let r = f(self);
        #[cfg(feature = "gc-trace")]
        {
            (self.heap.trace.in_c, self.heap.trace.call_hook) = was;
        }
        let _ = (in_c, call_hook);
        r
    }
}
