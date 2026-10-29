//! The Lua-to-Lua call the fast loop makes without `begin_call`.

use super::*;
use crate::runtime::puc::api::PucCollector;

impl Vm {
    /// Push the frame of a Lua function called from the fast loop (PUC
    /// `luaD_precall`) when that is all the call needs: no method JIT to
    /// try, fixed parameters, the stack big enough; the caller runs with no
    /// hook and no trace JIT. The new frame, or `None`, having done nothing
    /// but clear registers of the callee's, to leave the call to
    /// `begin_call` (which grows a frame stack with no room left, where
    /// running out of memory can be raised). `PRE` is false for 5.3 on,
    /// where neither the 5.2 call check nor 5.1's whole-window clear can
    /// apply, so that the fast loop does not read the dialect for them.
    #[cfg_attr(luna_hot, inline(always))]
    pub(super) fn push_lua_frame_fast<const PRE: bool>(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Option<*mut Frame> {
        let p = cl.proto;
        let base = func_slot + 1;
        let need = base as usize + p.max_stack as usize;
        if self.jit.gate
            || p.is_vararg
            || p.has_compat_vararg_arg
            || base + nargs + p.max_stack as u32 >= self.g.stack_last
            || self.stack.len() < need
        {
            return None;
        }
        // 5.2 checks for a collection on every call: one due is
        // `begin_call`'s to make (and to log, in the trace)
        if PRE && self.heap.rules().check_on_call() && !self.checkpoint_idle() {
            return None;
        }
        // as `push_frame`: missing parameters are nil, and 5.1 clears the
        // whole window
        let kept = nargs.min(p.num_params as u32);
        let end = if PRE && self.version == LuaVersion::Lua51 {
            need
        } else {
            base as usize + p.num_params as usize
        };
        // SAFETY: `need <= stack.len()` was checked above and `base + kept
        // <= end <= need` (the verifier keeps `num_params <= max_stack`)
        unsafe {
            self.stack
                .get_unchecked_mut((base + kept) as usize..end)
                .fill(Value::Nil);
        }
        // a frame stack with no room left, a level with no `CallInfo`
        // counted yet or 5.1's array full (`CallGuards::ci_hint`, at most
        // the room)
        if self.frames.len() as i32 >= self.g.ci_hint {
            return None;
        }
        let frame = CallFrame::Lua(self.fresh_frame(cl, base, func_slot, nresults));
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            frame,
        );
        self.assert_levels_counted();
        // SAFETY: a Lua frame was just pushed
        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(f) => Some(f),
            // SAFETY: see above
            CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
        }
    }

    /// A Lua frame for `cl` at `base`, about to run, with what the call
    /// that makes it left pending (its tag method, hook, tail calls).
    #[inline(always)]
    fn fresh_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        func_slot: u32,
        nresults: i32,
    ) -> Frame {
        Frame {
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
        }
    }
}

/// What [`Vm::return_fast`] did.
pub(super) enum Returned {
    /// nothing: the loop head's `Return` arm takes over
    No,
    /// to a metamethod's continuation, which sets `trap`
    ToMeta,
    /// to this Lua frame, now on top
    ToLua(*mut Frame),
}

impl Vm {
    /// `Return0` / `Return1` without the close and hook machinery (PUC
    /// `OP_RETURN0` / `OP_RETURN1`): when no return hook can fire, nothing
    /// in this frame needs closing and the caller is a Lua frame or a
    /// metamethod's continuation inside this activation, the return is the
    /// pop, the move of the (at most one) result and the result count that
    /// `complete_return` would do. `false`, having done nothing, otherwise.
    /// Without `WATCH` no hook is armed: arming one sets `trap`, which the
    /// fast loop leaves for its head before running anything else.
    /// `close` is the return's `k`: only then can the frame have open
    /// upvalues or to-be-closed slots (see `mark_closing_returns`).
    #[cfg_attr(luna_hot, inline(always))]
    pub(super) fn return_fast<const WATCH: bool>(
        &mut self,
        base: u32,
        abs_a: u32,
        nret: u32,
        entry_depth: usize,
        close: bool,
    ) -> Returned {
        let n = self.frames.len();
        if n <= entry_depth
            || n < 2
            || WATCH && self.hook.ret && self.hook_armed()
            || close
                && (self.open_upvals.last().is_some_and(|&(s, _)| s >= base)
                    || self.heap.has_dead_open()
                    || self.tbc.last().is_some_and(|&s| s >= base))
        {
            return Returned::No;
        }
        // not indexed: that reborrows every frame, the running one too,
        // whose pc the fast loop still writes on `No`
        // SAFETY: `n >= 2`, so slot `n - 2` holds a frame
        let caller_slot = unsafe { &mut *self.frames.as_mut_ptr().add(n - 2) };
        let caller: Option<*mut Frame> = match caller_slot {
            CallFrame::Lua(f) => Some(f),
            CallFrame::Cont(c) if matches!(c.kind, ContKind::Meta(_)) => None,
            CallFrame::Cont(_) => return Returned::No,
        };
        let to_meta = caller.is_none();
        let (func_slot, wanted) = match &self.frames[n - 1] {
            CallFrame::Lua(f) => (f.func_slot, f.nresults),
            // SAFETY: the running frame is on top, and it is a Lua frame
            CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
        };
        frames_pop_known(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            to_meta,
        );
        if nret == 1 {
            // SAFETY: both slots are in the returning frame's window or the
            // caller's, which the stack holds
            unsafe {
                let s = self.stack.as_mut_ptr();
                Value::copy_raw(s.add(func_slot as usize), s.add(abs_a as usize));
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
        match caller {
            Some(f) => Returned::ToLua(f),
            None => Returned::ToMeta,
        }
    }
}
