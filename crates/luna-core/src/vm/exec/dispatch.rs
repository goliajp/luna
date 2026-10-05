//! The interpreter loop head: trap handling, continuation frames, trace
//! dispatch, then the fast loop and the opcodes it hands back.

use super::*;

impl Vm {
    /// The loop head's slow path, taken while [`Vm::trap`] is set: tick the
    /// instruction budget, enforce the memory cap, then clear `trap` unless
    /// one of them, an armed hook or a continuation frame on top still needs
    /// the next pass through the loop head.
    #[inline]
    pub(super) fn trap_step(&mut self) -> Result<(), LuaError> {
        if let Some(b) = self.instr_budget.as_mut() {
            *b -= 1;
            if *b <= 0 {
                return Err(self.instr_budget_exhausted());
            }
        }
        if let Some(cap) = self.heap.mem_cap
            && self.heap.bytes() > cap
        {
            self.mem_cap_exceeded(cap)?;
        }
        self.trap = self.instr_budget.is_some()
            || self.heap.mem_cap.is_some()
            || self.hook_armed()
            || matches!(self.frames.last(), Some(CallFrame::Cont(_)));
        Ok(())
    }

    /// A continuation frame is on top: the call it protected has delivered
    /// its results (or a `__close` handler / yieldable metamethod finished).
    /// `Some` hands results out of this activation.
    #[inline(never)]
    pub(super) fn finish_cont(
        &mut self,
        nc: NativeCont,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        // a yieldable metamethod returned: complete the interrupted
        // instruction (PUC luaV_finishOp) and resume the running frame.
        if let ContKind::Meta(mc) = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            let result = if self.top > nc.func_slot {
                self.stack[nc.func_slot as usize]
            } else {
                Value::Nil
            };
            self.stack.truncate(nc.func_slot as usize);
            self.top = mc.saved_top;
            self.finish_meta(mc.action, result)?;
            return Ok(None);
        }
        // a __close handler returned successfully: discard its
        // results, restore `top` to the slot the handler was called
        // at (the surrounding frame's register window above this slot
        // must stay alloc'd — never truncate the underlying stack),
        // then continue the close chain (next slot, or fire
        // AfterClose). When the close ends an entry activation,
        // drive_close hands the results up to exec_with directly.
        if let ContKind::Close(cc) = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            let pending = cc.has_pending.then(|| self.stack[nc.func_slot as usize]);
            self.top = nc.func_slot;
            if let Some(vals) = self.drive_close(cc.from, pending, cc.after, entry_depth)? {
                return Ok(Some(vals));
            }
            return Ok(None);
        }
        // __pairs returned: normalize its results to exactly the
        // dialect's count (iterator, state, control, and on 5.5 the
        // closing value) at pairs's slot, where the metamethod was
        // called, and hand them to pairs's caller.
        if let ContKind::Pairs = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            let total = crate::vm::builtins::pairs_mm_results(self) as u32;
            let need = (nc.func_slot + total) as usize;
            if self.stack.len() < need {
                self.grow_stack_or_abort(need);
            }
            // the metamethod ran one slot above pairs's own
            let first = nc.func_slot + 1;
            let n = (self.top - first).min(total);
            for i in 0..n {
                self.stack[(nc.func_slot + i) as usize] = self.stack[(first + i) as usize];
            }
            for s in (nc.func_slot + n)..(nc.func_slot + total) {
                self.stack[s as usize] = Value::Nil;
            }
            self.top = nc.func_slot + total;
            if self.frames.len() < entry_depth {
                return Ok(Some(self.take_results(nc.func_slot)));
            }
            self.finish_results(nc.func_slot, total, nc.nresults);
            return Ok(None);
        }
        if let ContKind::Host(hc) = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            return self.finish_host_cont(nc, hc, entry_depth);
        }
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        self.pcall_depth -= 1;
        // f's results sit at nc.func_slot+1.. (f was called one slot
        // above the continuation), so writing `true` at the slot makes
        // `true, results…` already contiguous.
        let nret = self.top - (nc.func_slot + 1);
        self.stack[nc.func_slot as usize] = Value::Bool(true);
        let total = 1 + nret;
        self.top = nc.func_slot + total;
        if self.frames.len() < entry_depth {
            return Ok(Some(self.take_results(nc.func_slot)));
        }
        self.finish_results(nc.func_slot, total, nc.nresults);
        Ok(None)
    }

    pub(super) fn run(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        // the host may have set a budget, a cap or a hook since the last run
        self.trap = true;
        let pre53 = self.version() <= LuaVersion::Lua53;
        let dbl = self.version() <= LuaVersion::Lua52;
        loop {
            if self.trap {
                self.trap_step()?;
            }
            // A continuation frame on top means the call it protected just
            // delivered its results. Pushing a continuation, or popping down
            // to one, sets `trap` (`frames_push_sync` / `frames_pop_sync`), so
            // the loop head looks for one only under `trap`.
            if self.trap
                && let Some(&CallFrame::Cont(nc)) = self.frames.last()
            {
                if let Some(vals) = self.finish_cont(nc, entry_depth)? {
                    return Ok(vals);
                }
                continue;
            }
            debug_assert!(
                matches!(self.frames.last(), Some(CallFrame::Lua(_))),
                "a continuation frame on top with `trap` clear"
            );
            // GC runs only at the allocation safe points below (PUC's
            // `luaC_checkGC` sites), each with a precise `gc_top`; the loop head
            // no longer collects, so a stale full-window `gc_top` cannot leak in.
            //
            // Hot-path frame fetch: the Cont arm above continues the loop,
            // so reaching here means `frame_peek` is the Lua frame. Reuse it
            // rather than re-fetching `self.frames.last()`.
            // SAFETY: the running thread always has a frame here, and it is a
            // Lua frame: a continuation on top was consumed above
            let f = match unsafe { self.frames.last().unwrap_unchecked() } {
                CallFrame::Lua(f) => f,
                // SAFETY: see above
                CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
            };
            let cl = f.closure;
            let base = f.base;
            let pc = f.pc;
            let oldpc = f.hook_oldpc;

            // SAFETY: `pc` is bounded by the compiler against `proto.code.len()`
            // — every branch / call op only sets `pc` to a valid index, and
            // function entry initialises pc=0 with a non-empty body. PUC's
            // `vmfetch` uses the equivalent unchecked load.
            let inst = unsafe { *cl.proto.code.get_unchecked(pc as usize) };

            // Trace recording append + close detection.
            // Gated on `trace_jit_enabled` + `active_trace.is_some()`
            // so default dispatch keeps a single not-taken branch.
            //
            // - At the head PC with a non-empty record, the trace has
            //   looped back to its start: mark `closed = true` and
            //   take the record for compile + cache.
            // - Otherwise, capture the op. If the record overflows
            //   MAX_TRACE_LEN, abort by dropping it.
            // read once: with the trace JIT off this is the only JIT test an
            // instruction makes
            let trace_on = self.jit.trace_enabled;
            if trace_on && self.jit.active_trace.is_some() {
                self.trace_record_step(cl, pc, inst, base);
            }

            // Trace JIT dispatcher.
            //
            // When the dispatch loop is about to execute the op at
            // `pc` and there's a `numeric_only` CompiledTrace cached
            // for that `head_pc`, marshal the live regs into an
            // i64 buffer, jump into the trace, and resume the
            // interpreter at the returned continuation PC.
            //
            // Skipped when `trace_jit_enabled` is false or the Proto
            // holds no trace the lookup could admit
            // (`has_dispatchable_trace`); otherwise the lookup is a
            // borrow + scan over `cl.proto.traces`.
            //
            // Marshalling contract — only Int slots survive the
            // round-trip cleanly (the reg_state ABI is `*mut i64`
            // with no tag info). Any non-Int slot in the affected
            // window forces a skip; interp takes over for one op
            // and the back-edge brings us back to try again next
            // pass (slots that were Nil/Float at one moment can
            // settle to Int by the time the next back-edge fires).
            //
            // A trace that comes back with `vm.jit.pending_err`
            // parked is treated as a deopt: clear the err, leave
            // the stack as the trace wrote it, and let the
            // interpreter run from the same `pc`. The trace itself
            // is left cached — a future entry might find no
            // metatable in the way and succeed.
            // Single Rc<CompiledTrace> clone instead of per-field Rc
            // clones: proto.traces is Vec<Rc<CompiledTrace>>; the
            // dispatcher clones ONE Rc and reads fields via auto-deref.
            // One-shot consume of the
            // `suppress_downrec_admit_once` flag. Set by the
            // downrec post-invoke arm below when it force-deopts the
            // trace (caller-pc guard miss OR cycle-budget exhausted)
            // so the NEXT interpreter loop iteration skips the
            // downrec admit, lets interp run the op at `head_pc`,
            // advances `pc` past `head_pc`, and breaks the otherwise-
            // infinite admit loop. Reading + clearing here means a
            // single dispatch tick consumes the suppression — the
            // following tick re-admits naturally (with the budget
            // also reset by the deopt site).
            // The one-shot suppression only matters where a downrec trace
            // could be admitted, which needs the proto's flag.
            // Compiled code does not tick the instruction budget: while one
            // is armed, every loop stays in the interpreter.
            let admit =
                trace_on && cl.proto.has_dispatchable_trace.get() && self.instr_budget.is_none();
            let downrec_admit_blocked =
                admit && std::mem::take(&mut self.jit.suppress_downrec_admit_once);
            if admit && self.trace_dispatch(cl, pc, base, downrec_admit_blocked) {
                continue;
            }

            // PUC `vmfetch` increments savedpc BEFORE firing traceexec, so
            // hook code that consults `currentpc = savedpc - 1` lands on the
            // instruction now executing. luna mirrors that by advancing
            // `f.pc` to `pc + 1` before the hook block — local_at /
            // getinfo / line attribution all read f.pc, and the existing
            // `pc - 1` convention in those helpers then yields the current
            // instruction's pc (db.lua :696: local `A` visible at the
            // chunk's return line once OP_CLOSURE has advanced pc).
            //
            // Inline `top_frame_mut` for the hot path: top is guaranteed Lua
            // (cont frames drained above) so the and_then/Option layers are
            // dead weight.
            // SAFETY: the running thread has a frame while it executes
            // instructions
            let mut fr: *mut Frame = match unsafe { self.frames.last_mut().unwrap_unchecked() } {
                CallFrame::Lua(fmut) => {
                    fmut.pc = pc + 1;
                    fmut
                }
                _ => unreachable!("Cont frame at pc bump"),
            };

            // count + line hooks (PUC traceexec): before executing the
            // instruction. Skipped while the hook itself runs. `trap` is set
            // whenever a hook is armed, so the common case tests one byte.
            if self.trap && self.hook_armed() {
                self.exec_hooks(cl, pc, oldpc)?;
                // a hook runs Lua code, which can move `self.frames`
                // SAFETY: as above
                fr = match unsafe { self.frames.last_mut().unwrap_unchecked() } {
                    CallFrame::Lua(fmut) => fmut,
                    _ => unreachable!("Cont frame after a hook"),
                };
            }

            let mut heads = [crate::runtime::function::TRACE_HEADS_NONE;
                crate::runtime::function::TRACE_HEADS_CAP];
            let stay = !self.trap
                && (!trace_on
                    || self.jit.active_trace.is_none() && {
                        heads = cl.proto.trace_heads.get();
                        heads[0] != crate::runtime::function::TRACE_HEADS_MANY
                    });
            let fx = fast::Fast {
                fr,
                trace_on,
                pre53,
                entry_depth,
                stay,
                heads,
            };
            let watch = !stay || heads[0] != crate::runtime::function::TRACE_HEADS_NONE;
            let out = match (watch, trace_on, dbl) {
                (true, _, false) => self.run_fast::<true, true, false>(fx, inst, pc + 1)?,
                (false, true, false) => self.run_fast::<false, true, false>(fx, inst, pc + 1)?,
                (false, false, false) => self.run_fast::<false, false, false>(fx, inst, pc + 1)?,
                (true, _, true) => self.run_fast::<true, true, true>(fx, inst, pc + 1)?,
                (false, true, true) => self.run_fast::<false, true, true>(fx, inst, pc + 1)?,
                (false, false, true) => self.run_fast::<false, false, true>(fx, inst, pc + 1)?,
            };
            let inst = match out {
                fast::FastExit::Reload => continue,
                fast::FastExit::Slow(inst) => inst,
            };
            if let Some(vals) = self.run_slow(inst, entry_depth)? {
                return Ok(vals);
            }
            // the loop head reloads everything from the frame
        }
    }
}
