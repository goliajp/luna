//! The Lua-to-Lua call the fast loop makes without `begin_call`.

use super::*;

impl Vm {
    /// Push the frame of a Lua function called from the fast loop (PUC
    /// `luaD_precall` for a Lua function) when the frame is all the call
    /// needs: no method JIT to try, a function with fixed parameters and
    /// the stack already big enough. The caller runs with no hook and no
    /// trace JIT, so neither has anything to see. `false`, having done
    /// nothing, leaves the call to `begin_call`.
    #[inline(always)]
    pub(super) fn push_lua_frame_fast(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> bool {
        let p = cl.proto;
        let base = func_slot + 1;
        let need = base as usize + p.max_stack as usize;
        if self.jit.enabled
            || p.is_vararg
            || p.has_compat_vararg_arg
            || func_slot + 256 > MAX_LUA_STACK
            || self.stack.len() < need
        {
            return false;
        }
        // as `push_frame`: the window past the parameters starts out nil
        let kept = nargs.min(p.num_params as u32);
        // SAFETY: `need <= stack.len()` was checked above and `base + kept
        // <= base + num_params <= need` (the verifier keeps `num_params <=
        // max_stack`)
        unsafe {
            self.stack
                .get_unchecked_mut((base + kept) as usize..need)
                .fill(Value::Nil);
        }
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Lua(Frame {
                closure: cl,
                base,
                pc: 0,
                func_slot,
                nresults,
                hook_oldpc: u32::MAX,
                from_c: false,
                n_varargs: 0,
                tm: self.pending_tm.take(),
                is_hook: std::mem::take(&mut self.pending_is_hook),
                tailcalls: std::mem::take(&mut self.pending_tailcalls),
                ccmt: std::mem::take(&mut self.pending_ccmt),
            }),
        );
        true
    }
}
