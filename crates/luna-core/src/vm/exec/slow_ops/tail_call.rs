//! `OP_TAILCALL`.

use super::*;

impl Vm {
    /// A tail call: one of a Lua function replaces the running one; one of
    /// a native is a call like any other, whose results the `Return` after
    /// it returns. The parts before and after are functions of their own
    /// when unoptimised, off the stack under the call.
    #[cfg_attr(luna_hot, inline(always))]
    pub(super) fn op_tail_call(
        &mut self,
        inst: Inst,
        base: u32,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        let fr = *self.top_frame();
        let (func, abs, nargs, chain) = self.tail_call_target(inst, base)?;
        if let Value::Closure(cl) = func {
            return self.tail_call_lua(fr, cl, abs, nargs, entry_depth);
        }
        // Native (or __call-bearing) target: regular call. The
        // results land at `abs..self.top` and the next op (the
        // fallback `Op::Return`) forwards them. `wanted = -1`
        // because the caller will multret them through Return.
        // PUC's precallC gives the C call this tail call's own
        // `__call` count (5.5 extraargs); the chain was already
        // resolved above, so hand it over.
        self.pending_ccmt = chain as u8;
        self.begin_call(abs, Some(nargs), -1, false)?;
        Ok(None)
    }

    /// The tail call's function once a chain of `__call` metamethods is
    /// resolved in place, with its slot, the count of its arguments (above
    /// it) and the count of metamethods.
    #[cfg_attr(luna_hot, inline(always))]
    fn tail_call_target(
        &mut self,
        inst: Inst,
        base: u32,
    ) -> Result<(Value, u32, u32, u32), LuaError> {
        let abs = base + inst.a();
        let mut nargs = if inst.b() == 0 {
            self.top - (abs + 1)
        } else {
            inst.b() - 1
        };
        // A tail call pops this frame before begin_call, so a
        // non-callable target would lose its name/position. Report
        // it now (PUC reads funcname from the still-current ci),
        // while the frame is intact, for "(field 'x')"-style info.
        let mut func = self.stack[abs as usize];
        if !matches!(func, Value::Closure(_) | Value::Native(_))
            && self.get_mm(func, Mm::Call).is_nil()
        {
            // 5.3 on make sure of a slot for `__call` before finding none
            if self.version >= LuaVersion::Lua53 {
                self.check_lua_stack(abs + 1 + nargs, 1, true)?;
            }
            return Err(self.call_err_at(func, abs + 1 + nargs));
        }
        // PUC `luaD_pretailcall` resolves a chain of `__call`
        // metamethods *in place* before deciding whether to
        // collapse this frame. Without that, each __call hop
        // would push a fresh Lua frame and a 10000-deep
        // tail-recursion through a 100-deep __call chain
        // (5.4 calls.lua :172) blows up. Mirror the PUC loop:
        // shift args right, install the handler at `abs`, retry.
        // Chain depth limit matches the call-site `begin_call`
        // version cap (5.5 calls.lua :223 — 15 max, then "too
        // long"; 16th wrap fails the call). An infinite
        // self-referential `__call` would otherwise spin.
        let chain_cap = if self.version >= LuaVersion::Lua55 {
            15
        } else {
            MAX_CCMT
        };
        let mut chain = 0u32;
        while !matches!(func, Value::Closure(_) | Value::Native(_)) {
            let mm = self.get_mm(func, Mm::Call);
            if mm.is_nil() || self.call_mm_unusable(mm) {
                return Err(self.call_err_at(func, abs + 1 + nargs));
            }
            chain += 1;
            if chain > chain_cap {
                return Err(self.rt_err("'__call' chain too long"));
            }
            let end = (abs + 1 + nargs) as usize;
            self.grow_stack(end + 1)?;
            for i in (0..=nargs).rev() {
                self.stack[(abs + 1 + i) as usize] = self.stack[(abs + i) as usize];
            }
            self.stack[abs as usize] = mm;
            nargs += 1;
            self.top = abs + 1 + nargs;
            func = mm;
        }
        // PUC's tail-call collapse is Lua→Lua only. A tail call to
        // a C function runs the C function under the *current* Lua
        // activation (no frame fold — a C frame has nothing to
        // collapse into); after the C function returns, the
        // calling Lua function returns those results normally.
        // Mirror that: keep our Lua frame on the stack, call the
        // target through `begin_call(abs, …)` as a regular call,
        // and let the fallback `Op::Return` that the compiler
        // emits right after `Op::TailCall` forward the results.
        // 5.1 closure.lua :177's `return getfenv()` from inside
        // foo needs level 1 to resolve to foo, not to the
        // thread's globals fallback that happens when no Lua
        // frame is on the stack.
        Ok((func, abs, nargs, chain))
    }

    /// The tail call of a Lua function: it takes the place of the running
    /// one, `fr`.
    #[cfg_attr(luna_hot, inline(always))]
    fn tail_call_lua(
        &mut self,
        fr: Frame,
        cl: Gc<crate::runtime::LuaClosure>,
        abs: u32,
        nargs: u32,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        // PUC checks the stack for the callee before moving it down
        // over this frame, from where it was called; 5.4 on count the
        // slots a vararg caller's extra arguments free (`delta`)
        let delta = if self.version >= LuaVersion::Lua54 && fr.closure.proto.is_vararg {
            fr.n_varargs + u32::from(fr.closure.proto.num_params) + 1
        } else {
            0
        };
        let fsize = u32::from(cl.proto.max_stack).saturating_sub(delta);
        self.check_lua_stack(abs + 1 + nargs, fsize, true)?;
        // 5.1's `CallInfo` of the callee is taken before this one goes
        if self.g.frame_size != u32::MAX && self.frames_in_use() >= self.g.frame_size {
            self.grow_frames()?;
        }
        // 5.2 and 5.3 too take one, from the list above this one
        if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
            self.extend_ci_to(self.ci_in_use() + 1);
        }
        if self.version <= LuaVersion::Lua53 && self.call_hook_armed() {
            self.tail_call_hook(cl, abs, nargs)?;
        }
        self.close_slots_at(fr.base, None, false)?;
        for i in 0..=nargs {
            self.stack[(fr.func_slot + i) as usize] = self.stack[(abs + i) as usize];
        }
        // Clear the slot range that's now stranded by the tail-call
        // collapse. The args were copied to `[fr.func_slot..fr.func_slot+
        // nargs+1)`; the source slots `[abs..abs+nargs+1)` still hold the
        // same `Value::Closure / Value::Str / ...` entries, but they're
        // past the new call's window. Without this clear, a later GC with
        // wider gc_top would mark stale pointers there (same hazard the
        // finish_results slot-clear closes for the Op::Return path).
        let new_top_lower_bound = fr.func_slot + nargs + 1;
        let prev_top = (self.top as usize).min(self.stack.len());
        if (new_top_lower_bound as usize) < prev_top {
            for slot in &mut self.stack[new_top_lower_bound as usize..prev_top] {
                *slot = Value::Nil;
            }
        }
        // PUC `CIST_TAIL`: the new Lua activation inherits
        // the popped frame's tailcalls count plus one for
        // this collapse. 5.1 db.lua :372 hammers 30000
        // recursive tail calls and expects to see the
        // synthetic tail level for every one of them.
        self.pending_tailcalls = fr.tailcalls.saturating_add(1);
        self.pending_ccmt = fr.ccmt;
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        let called = self.begin_call(fr.func_slot, Some(nargs), fr.nresults, false);
        // an error before the frame was pushed leaves the flag unread
        self.tail_hook_fired = false;
        if !called? && self.frames.len() < entry_depth {
            // a native completed what was this function's result
            return Ok(Some(self.take_results(fr.func_slot)));
        }
        Ok(None)
    }
}
