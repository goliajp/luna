//! Protected calls made from Rust, with the error bookkeeping saved around them.

use crate::runtime::Value;
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

    /// [`Vm::call_protected`] with `handler` as the message handler that is
    /// running while `f` runs (`L->errfunc` during `luaG_errormsg`'s call).
    pub(super) fn call_protected_with(
        &mut self,
        f: Value,
        args: &[Value],
        handler: Option<Value>,
        at: Option<u32>,
    ) -> Result<Vec<Value>, crate::vm::error::LuaError> {
        let running = std::mem::replace(&mut self.msgh_running, handler);
        let floor = std::mem::replace(&mut self.msgh_floor, self.frames.len());
        let applied = self.msgh_applied.take();
        let traceback = self.error_traceback.take();
        let natives = self.errored_natives.take();
        let keep = std::mem::replace(&mut self.keep_error_traceback, false);
        let r = self.call_value_impl(f, args, false, at);
        self.keep_error_traceback = keep;
        self.msgh_running = running;
        self.msgh_floor = floor;
        self.msgh_applied = applied;
        self.error_traceback = traceback;
        self.errored_natives = natives;
        r
    }
}
