//! Raising an error from a C API call, for the C side to throw.

use super::*;

impl Api<'_> {
    /// Raise `e` from this call: its object goes on top of the stack and
    /// the C side throws `status` once the Rust side has returned.
    pub(in crate::capi) fn raise_status(&mut self, e: LuaError, status: c_int) {
        self.push(e.0);
        let l = self.l;
        let g = self.g();
        g.err_from = l;
        g.raised = status;
    }

    pub(in crate::capi) fn raise(&mut self, e: LuaError) {
        self.raise_status(e, LUA_ERRRUN);
    }

    /// Raise a runtime error with message `msg`, with no position (PUC
    /// `luaG_runerror` from inside a C function).
    pub(in crate::capi) fn raise_msg(&mut self, msg: &str) {
        let e = match self.vm.host_str(msg.as_bytes()) {
            Ok(s) => LuaError(s),
            Err(e) => e,
        };
        self.raise(e);
    }

    /// Whether this call raised.
    pub(in crate::capi) fn raised(&mut self) -> bool {
        self.g().raised != 0
    }

    /// Push the message `msg` for a call that returns status `status`; when
    /// the message cannot be made, the memory error goes there instead and
    /// the status is `LUA_ERRMEM`.
    pub(in crate::capi) fn push_status(&mut self, msg: &[u8], status: c_int) -> c_int {
        match self.str(msg) {
            Ok(m) => {
                self.push(m);
                status
            }
            Err(e) => {
                self.push(e.0);
                LUA_ERRMEM
            }
        }
    }

    /// `bytes` as a string value; a memory error when it cannot be made.
    pub(in crate::capi) fn str(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        self.vm.host_str(bytes)
    }
}
