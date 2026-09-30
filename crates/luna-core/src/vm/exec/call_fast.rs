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

impl Vm {
    /// `Return0` / `Return1` without the close and hook machinery (PUC
    /// `OP_RETURN0` / `OP_RETURN1`): when no return hook can fire, nothing
    /// in this frame needs closing and the caller is a Lua frame or a
    /// metamethod's continuation inside this activation, the return is the
    /// pop, the move of the (at most one) result and the result count that
    /// `complete_return` would do. `false`, having done nothing, otherwise.
    #[inline(always)]
    pub(super) fn return_fast(
        &mut self,
        base: u32,
        abs_a: u32,
        nret: u32,
        entry_depth: usize,
    ) -> bool {
        let n = self.frames.len();
        if n <= entry_depth
            || n < 2
            || self.hook.ret && self.hook_armed()
            || self.open_upvals.last().is_some_and(|&(s, _)| s >= base)
            || self.tbc.last().is_some_and(|&s| s >= base)
        {
            return false;
        }
        // SAFETY: `n >= 2`
        let to_meta = match unsafe { self.frames.get_unchecked(n - 2) } {
            CallFrame::Lua(_) => false,
            CallFrame::Cont(c) if matches!(c.kind, ContKind::Meta(_)) => true,
            CallFrame::Cont(_) => return false,
        };
        // SAFETY: the running frame is on top, and it is a Lua frame
        let (func_slot, wanted) = match unsafe { self.frames.get_unchecked(n - 1) } {
            CallFrame::Lua(f) => (f.func_slot, f.nresults),
            // SAFETY: see above
            CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
        };
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        if nret == 1 {
            // SAFETY: both slots are in the returning frame's window or the
            // caller's, which the stack holds
            unsafe {
                let s = self.stack.as_mut_ptr();
                *s.add(func_slot as usize) = *s.add(abs_a as usize);
            }
        }
        if to_meta || wanted < 0 {
            self.top = func_slot + nret;
        } else {
            let w = wanted as u32;
            if nret < w {
                self.pad_results(func_slot + nret, func_slot + w);
            } else if nret > w {
                // one result, none wanted
                self.stack[func_slot as usize] = Value::Nil;
            }
            self.top = func_slot + w;
        }
        true
    }
}
