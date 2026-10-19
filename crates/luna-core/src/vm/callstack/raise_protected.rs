//! Protected calls made from Rust, with the error bookkeeping saved around them.

use crate::runtime::Value;
use crate::runtime::mem::LVec;
use crate::vm::exec::Vm;

impl Vm {
    /// `call_value` as a protected call made from Rust (PUC `lua_pcall`
    /// with no handler): errors inside it do not reach the handler of an
    /// enclosing xpcall, and its error bookkeeping does not outlive it.
    pub(crate) fn call_protected(
        &mut self,
        f: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, crate::vm::error::LuaError> {
        self.call_protected_with(f, args, None, None)
    }

    /// [`Vm::call_protected`] placed at slot `at` of the running native's
    /// stack instead of at that native's top.
    pub(crate) fn call_protected_at(
        &mut self,
        f: Value,
        args: &[Value],
        at: u32,
    ) -> Result<Vec<Value>, crate::vm::error::LuaError> {
        self.call_protected_with(f, args, None, Some(at))
    }

    /// [`Vm::call_protected`] with `handler` as the message handler that is
    /// running while `f` runs (`L->errfunc` during `luaG_errormsg`'s call).
    pub(super) fn call_protected_with(
        &mut self,
        f: Value,
        args: &[Value],
        handler: Option<Value>,
        at: Option<u32>,
    ) -> Result<Vec<Value>, crate::vm::error::LuaError> {
        self.call_protected_keeping(f, args, handler, at).0
    }

    /// [`Vm::call_protected_with`], with the natives the error it ended in
    /// was raised by (see [`Vm::note_errored_native`]).
    pub(super) fn call_protected_keeping(
        &mut self,
        f: Value,
        args: &[Value],
        handler: Option<Value>,
        at: Option<u32>,
    ) -> (
        Result<Vec<Value>, crate::vm::error::LuaError>,
        LVec<super::ErroredNative>,
    ) {
        let running = std::mem::replace(&mut self.msgh_running, handler);
        let floor = std::mem::replace(&mut self.msgh_floor, self.frames.len());
        let applied = self.msgh_applied.take();
        let traceback = self.error_traceback.take();
        let natives = self.errored_natives.take();
        let keep = std::mem::replace(&mut self.keep_error_traceback, false);
        // a protected call made from Rust has no continuation: code under
        // it cannot yield (a finalizer's `luaD_callnoyield`, a message
        // handler's, `lua_pcall` with none)
        self.nny += 1;
        let r = self.call_value_impl(f, args, false, at);
        self.nny -= 1;
        self.keep_error_traceback = keep;
        self.msgh_running = running;
        self.msgh_floor = floor;
        self.msgh_applied = applied;
        self.error_traceback = traceback;
        let inner = std::mem::replace(&mut self.errored_natives, natives);
        (r, inner)
    }
}
