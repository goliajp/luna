//! The two kinds of call [`Vm::begin_call`] starts, as functions of their
//! own in an unoptimised build: only the one taken is then on the stack
//! under the call it makes.

use super::*;

impl Vm {
    /// [`lua_call`] as a function of its own (an unoptimised build).
    #[cfg(not(luna_hot))]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin_lua_call(
        &mut self,
        cl: Gc<crate::runtime::LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        from_c: bool,
        tailcalls: u32,
        tail_ccmt: u8,
        chain: u32,
    ) -> Result<bool, LuaError> {
        lua_call!(
            self, cl, func_slot, nargs, nresults, from_c, tailcalls, tail_ccmt, chain
        )
    }

    /// [`native_call`] as a function of its own (an unoptimised build),
    /// what comes before the native runs in one of its own too: its locals
    /// are then off the stack while the native runs.
    #[cfg(not(luna_hot))]
    pub(super) fn begin_native_call(
        &mut self,
        nc: Gc<crate::runtime::NativeClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        tail_ccmt: u8,
        chain: u32,
    ) -> Result<bool, LuaError> {
        if let Some(done) = self.native_enter(nc, func_slot, nargs, nresults, tail_ccmt, chain)? {
            return Ok(done);
        }
        let nret = self.invoke_native(nc, func_slot, nargs)?;
        self.finish_native_call(func_slot, nargs, nret, nresults)?;
        Ok(false)
    }

    #[cfg(not(luna_hot))]
    fn native_enter(
        &mut self,
        nc: Gc<crate::runtime::NativeClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        tail_ccmt: u8,
        chain: u32,
    ) -> Result<Option<bool>, LuaError> {
        native_call!(
            apart, self, nc, func_slot, nargs, nresults, tail_ccmt, chain
        )
    }
}
