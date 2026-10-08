//! The opcodes the fast loop hands back to the loop head.

use super::*;

impl Vm {
    /// Run an opcode the fast loop handed back. `Some` carries the results
    /// once the call that `entry_depth` started has returned.
    #[inline(always)]
    pub(super) fn run_slow(
        &mut self,
        inst: Inst,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        // the fast loop may have called or returned into another frame
        let &Frame {
            closure: cl, base, ..
        } = self.top_frame();
        match inst.op() {
            Op::LoadKx
            | Op::NewTable
            | Op::SetList
            | Op::Pow
            | Op::Concat
            | Op::ForPrep
            | Op::ForPrep55
            | Op::TForPrep
            | Op::TForPrep53
            | Op::TForPrep55
            | Op::Closure
            | Op::Vararg
            | Op::GetVarg => self.run_frame_op(inst)?,
            Op::Close => {
                // Yieldable: drive __close handlers through the
                // interpreter loop so a coroutine.yield() inside a
                // handler suspends cleanly (locals.lua block-end yield).
                // `drive_close` parks the handler call at `self.top`, so
                // raise `top` past this frame's full register window
                // first — a goto out of a nested for-loop can fire
                // OP_Close while `self.top` still sits at the inner
                // body's working top, which would let `push_frame`'s
                // wipe clobber the outer tbc slot before it could be
                // closed (locals.lua:1219 nested-for goto regression).
                self.top = self.top.max(base + cl.proto.max_stack as u32);
                let _ = self.begin_close(base + inst.a(), None, AfterClose::Block, entry_depth)?;
            }
            Op::Tbc => {
                self.register_tbc(base + inst.a())?;
            }
            Op::TailCall => return self.op_tail_call(inst, base, entry_depth),
            Op::Return | Op::Return0 | Op::Return1 => {
                return self.op_return(inst, base, entry_depth);
            }
            Op::TForCall | Op::TForCall53 | Op::TForCall55 => {
                let lay = inst.op().for_layout().expect("a loop op");
                let abs = base + inst.a();
                let need = (abs + lay.call_end()) as usize;
                if self.stack.len() < need {
                    self.grow_stack_or_abort(need);
                }
                // the iterator, the state and the control, copied to where
                // the call runs (the control first: in 5.5 it is there)
                let call = (abs + lay.var()) as usize;
                self.stack[call + 2] = self.stack[(abs + lay.control()) as usize];
                self.stack[call + 1] = self.stack[(abs + 1) as usize];
                self.stack[call] = self.stack[abs as usize];
                let nvars = inst.c() as i32;
                self.begin_call(call as u32, Some(2), nvars, false)?;
            }
            Op::ExtraArg => unreachable!("EXTRAARG executed directly"),
            op => unreachable!("{op:?} is run by the fast loop"),
        }
        Ok(None)
    }

    #[inline(always)]
    fn op_tail_call(
        &mut self,
        inst: Inst,
        base: u32,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        let fr = *self.top_frame();
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
            if self.stack.len() < end + 1 {
                self.grow_stack_or_abort(end + 1);
            }
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
        if let Value::Closure(cl) = func {
            if self.version <= LuaVersion::Lua53 && self.call_hook_armed() {
                self.tail_call_hook(cl, abs, nargs)?;
            }
            self.close_slots(fr.base, None)?;
            for i in 0..=nargs {
                self.stack[(fr.func_slot + i) as usize] = self.stack[(abs + i) as usize];
            }
            // Clear the slot range that's now
            // stranded by the tail-call collapse. The args
            // were copied to `[fr.func_slot..fr.func_slot+
            // nargs+1)`; the source slots `[abs..abs+
            // nargs+1)` still hold the same `Value::Closure
            // / Value::Str / ...` entries, but they're past
            // the new call's window. Without this clear, a
            // later GC with wider gc_top would mark stale
            // pointers there (same hazard the
            // finish_results slot-clear closes for the
            // Op::Return path).
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
        } else {
            // Native (or __call-bearing) target: regular call. The
            // results land at `abs..self.top` and the next op (the
            // fallback `Op::Return`) forwards them. `wanted = -1`
            // because the caller will multret them through Return.
            // PUC's precallC gives the C call this tail call's own
            // `__call` count (5.5 extraargs); the chain was already
            // resolved above, so hand it over.
            self.pending_ccmt = chain as u8;
            self.begin_call(abs, Some(nargs), -1, false)?;
        }
        Ok(None)
    }

    #[inline(always)]
    fn op_return(
        &mut self,
        inst: Inst,
        base: u32,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        let (abs_a, nret) = match inst.op() {
            Op::Return0 => (base + inst.a(), 0),
            Op::Return1 => (base + inst.a(), 1),
            _ => {
                let abs_a = base + inst.a();
                let nret = if inst.b() == 0 {
                    self.top - abs_a
                } else {
                    inst.b() - 1
                };
                (abs_a, nret)
            }
        };
        // close before moving results: __close handlers run above
        // the stack top, so the result region [abs_a..abs_a+nret)
        // stays intact across any yields the close performs.
        // Fixed-count returns may leave `self.top` below the last
        // result slot (the compiler does not always re-bump it);
        // raise it past the result region so `drive_close` parks
        // the handler call *above* — landing at `self.top` would
        // otherwise clobber a result with the handler closure.
        self.top = self.top.max(abs_a + nret);
        if matches!(inst.op(), Op::Return0 | Op::Return1)
            && !matches!(
                self.return_fast::<true>(base, abs_a, nret, entry_depth, inst.k()),
                call_fast::Returned::No
            )
        {
            // done: the caller's frame is on top
        } else if let Some(vals) = self.begin_close(
            base,
            None,
            AfterClose::Return {
                abs_a,
                nret,
                from_native: false,
            },
            entry_depth,
        )? {
            return Ok(Some(vals));
        }
        Ok(None)
    }
}
