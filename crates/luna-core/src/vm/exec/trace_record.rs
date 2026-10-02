//! Trace recording in the dispatch loop: the instruction about to run is
//! appended to the active recording, which closes once the loop comes
//! back to its head.

use super::*;

impl Vm {
    /// Record `inst` (at `pc` of `cl`, frame `base`) into the active trace,
    /// or close the trace when the loop has returned to its head.
    #[inline(never)]
    pub(super) fn trace_record_step(&mut self, cl: Gc<LuaClosure>, pc: u32, inst: Inst, base: u32) {
        // Depth tracking. The trace head's frame is
        // at index `recording_frame_base`; every Op::Call that
        // pushes a new frame bumps the live depth, every
        // Op::Return that pops one decrements it.
        //
        // **Three clean-close conditions**:
        // - `at_head`: cur_depth == 0 AND about-to-execute the
        //   trace's head_pc on its head_proto (loop closed back
        //   to start). Same for loop-triggered and call-triggered
        //   traces, so a call-triggered trace does not close on
        //   the first re-entry (that would leave fib's body at 7
        //   depth=0 ops); it inlines up to MAX_INLINE_DEPTH
        //   levels before any close.
        // - `returned_past_head`: trace head's frame is gone
        //   (callee returned past it, or the call-trigger
        //   started a recording inside a callee that has now
        //   returned). Whatever ops were recorded form the
        //   trace body; the lowerer treats the partial trace
        //   the same as InlineAbort.
        // - `depth_cap_hit`: cur_depth > MAX_INLINE_DEPTH.
        //   Recording any deeper would just bloat the IR; close
        //   with the body we have. Lowerer's existing length
        //   gate + InlineAbort path handles short bodies.
        let returned_past_head = self.frames.len() <= self.jit.recording_frame_base;
        let cur_depth = if returned_past_head {
            0
        } else {
            self.frames.len() - 1 - self.jit.recording_frame_base
        };
        let depth_cap_hit = cur_depth > crate::jit::trace::MAX_INLINE_DEPTH as usize;
        if !returned_past_head {
            self.note_result_tag(cl, base, cur_depth);
        }
        let rec = self.jit.active_trace.as_mut().expect("just checked Some");
        let at_head_loop = cur_depth == 0
            && !rec.ops.is_empty()
            && !returned_past_head
            && std::ptr::eq(cl.proto.as_ptr(), rec.head_proto.as_ptr())
            && pc == rec.head_pc;
        // Self-link cycle catch (mirrors LuaJIT's
        // `check_call_unroll` at `lj_record.c:1869`). Trips when:
        //   1. We're about to execute the head_pc on head_proto
        //      at depth > 0 (we're re-entering the trace head
        //      from inside an inlined recursion level — UpRec).
        //   2. The count of ancestor frames in the recording
        //      window that share `head_proto` exceeds
        //      [`RECUNROLL_THRESHOLD`] (default 2).
        // For fib(N): head_pc=0, head_proto=fib. After 2 inline
        // recursion levels are captured, the recorder enters
        // the 3rd nested fib frame, sees cur_depth=3 > 2, and
        // trips this catch — closing with `SelfRecKind::UpRec`.
        // The lowerer's `TraceEnd::SelfLink` tail emits the
        // bump-base + branch-to-self loop body.
        //
        // TailRec vs UpRec: LJ distinguishes via
        // `framedepth + retdepth == 0`. luna doesn't track
        // retdepth separately; cur_depth == 0 with a non-empty
        // call chain in tail position is rare (would require
        // explicit Lua TCO). We use cur_depth > 0 as the UpRec
        // condition (fib's case); cur_depth == 0 with positive
        // ancestor count would route to TailRec, but luna's
        // recorder doesn't currently produce that shape because
        // tail-call elision pops the caller frame and we'd
        // hit `at_head_loop` instead.
        let self_link_trip: Option<crate::jit::trace::SelfRecKind> = {
            if self.jit.self_link_enabled
                && !returned_past_head
                && std::ptr::eq(cl.proto.as_ptr(), rec.head_proto.as_ptr())
                && pc == rec.head_pc
                && cur_depth > 0
            {
                // Count ancestor frames sharing head_proto.
                // self.frames[recording_frame_base..] currently
                // includes the just-pushed frame at the top
                // (the one about to execute head_pc). Ancestors
                // = the slice excluding the top frame.
                let head_proto_ptr = rec.head_proto.as_ptr();
                let last_idx = self.frames.len() - 1;
                let mut count = 0usize;
                for i in self.jit.recording_frame_base..last_idx {
                    if let CallFrame::Lua(f) = &self.frames[i]
                        && std::ptr::eq(f.closure.proto.as_ptr(), head_proto_ptr)
                    {
                        count += 1;
                    }
                }
                if count > crate::jit::trace::RECUNROLL_THRESHOLD {
                    // cur_depth > 0 → UpRec (fib pattern).
                    // cur_depth == 0 wouldn't reach this arm.
                    Some(crate::jit::trace::SelfRecKind::UpRec)
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some(kind) = self_link_trip {
            // SelfLink relax for self-recursive patterns at frame
            // depth >= 2.
            //
            // Stamping `self_link_kind` unconditionally at the
            // head_pc re-entry would never dispatch: the
            // `downrec_close` marker can only fire from the
            // depth>0 Op::Return path (`rec.retfs` chain),
            // which never reaches the recorder for fib(28)-like
            // shapes that hit the SelfLink cycle catch BEFORE
            // any base-case Return — leaving `downrec_close`
            // None and routing the trace through the safe
            // `dispatchable=false` `"self-link-retf-r1"` path.
            //
            // So when the SelfLink trip fires AND
            // `cur_depth >= 2` (the count > RECUNROLL_THRESHOLD
            // gate already requires this — kept explicit as a
            // safety floor), route the close through `downrec_
            // close` INSTEAD of `self_link_kind`. The recorder
            // synthesises the close marker from the most
            // recent Op::Call at depth `cur_depth - 1`:
            //   - `return_pc` = `call.pc + 1` (caller's resume
            //     PC after the recursive call returns; mirror
            //     of the `caller_pc` derivation at the
            //     depth>0 Op::Return capture path below).
            //   - `target_proto` = `call.proto` (caller's
            //     proto; equals `rec.head_proto` for self-
            //     recursion).
            //   - `depth_delta` = `1` (the recorder always
            //     unrolls one level; the Op::Return path uses
            //     the same constant).
            //
            // The lowerer's `end_idx` picker routes through
            // `TraceEnd::DownRec` ahead of the `self_link_kind`
            // arm and emits the stitch-sentinel +
            // caller-pc-guard scaffold. A single-candidate guard
            // chain (this path produces 1 caller_pc candidate
            // because `rec.retfs` is empty) keeps
            // `dispatchable=false` + `"downrec-stitch-pending"`
            // label (the lowerer requires
            // `multi_way_candidate_count >= 2`). Net behaviour:
            // trace compiles under DownRec routing; interp runs
            // the recursion naturally.
            //
            // The `cur_depth >= 2` gate is automatically
            // satisfied by the count > RECUNROLL_THRESHOLD=2
            // trip condition (3 ancestor frames sharing
            // head_proto implies cur_depth >= 3), kept
            // explicit so a future RECUNROLL_THRESHOLD tweak
            // doesn't silently flip shallow-recursion
            // shapes (cur_depth == 1) onto the DownRec arm.
            //
            // The recorded body still uses depth-baked
            // op_offsets[] addressing, so this is routing
            // scaffolding and gives no speedup by itself.
            let _ = kind;
            let relaxed_to_downrec = cur_depth >= 2 && rec.downrec_close.is_none() && {
                let caller_depth_u8 = (cur_depth - 1) as u8;
                if let Some(call_op) = rec.ops.iter().rev().find(|r| {
                    r.inline_depth == caller_depth_u8
                        && matches!(r.inst.op(), crate::vm::isa::Op::Call)
                }) {
                    rec.downrec_close = Some(crate::jit::trace::DownRecClose {
                        return_pc: call_op.pc + 1,
                        target_proto: call_op.proto,
                        depth_delta: 1,
                    });
                    true
                } else {
                    false
                }
            };
            if relaxed_to_downrec {
                // Close-cause taxonomy: tag the lift so
                // probes can tally the fire rate. Mirrors
                // the `"downrec-restart"` bump for the
                // depth>0 Op::Return path (different trip
                // origin, same downstream routing). The
                // existing `"self-link-retf-r1"` label still
                // fires for trips that DON'T relax (no
                // candidate Op::Call ancestor in rec.ops, or
                // cur_depth < 2) via the lowerer's
                // dispatch_off_reason mirror at the close
                // handler — kept as a regression safety net.
                self.jit
                    .counters
                    .bump_close_cause("selflink-yields-to-downrec");
            } else {
                rec.self_link_kind = Some(kind);
            }
        }
        let should_close =
            at_head_loop || returned_past_head || depth_cap_hit || self_link_trip.is_some();
        if should_close {
            self.trace_close_recording();
        } else {
            self.trace_record_push(cl, pc, inst, base, cur_depth);
        }
    }

    /// Note the tag the last recorded op left in its `R[A]`, when the
    /// instruction about to run is in the same frame (`base` of `cl` at
    /// `cur_depth`): the op has finished, and no call it made is running.
    fn note_result_tag(&mut self, cl: Gc<LuaClosure>, base: u32, cur_depth: usize) {
        let rec = self.jit.active_trace.as_mut().expect("recording");
        let Some(last) = rec.ops.last() else {
            return;
        };
        if last.inline_depth as usize != cur_depth
            || !std::ptr::eq(last.proto.as_ptr(), cl.proto.as_ptr())
        {
            return;
        }
        let i = rec.ops.len() - 1;
        let slot = (base + last.inst.a()) as usize;
        if let (Some(t), Some(v)) = (rec.result_tags.get_mut(i), self.stack.get(slot)) {
            *t = v.unpack().0;
        }
    }

    /// Append `inst` to the active recording at inline depth `cur_depth`.
    fn trace_record_push(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        inst: Inst,
        base: u32,
        cur_depth: usize,
    ) {
        if self.version <= LuaVersion::Lua52 && self.int_operand(inst, base) {
            // 5.1/5.2 integers stand for doubles; the trace lowering does
            // integer arithmetic in machine integers, without the rounding
            // and the -0 those need
            self.abort_recording("int-arith-on-doubles");
            return;
        }
        let rec = self.jit.active_trace.as_mut().expect("recording");
        // Depth-aware push at the
        // current `cur_depth`. The `depth_cap_hit` /
        // `returned_past_head` early-exit is handled by
        // the `should_close` branch above; reaching here
        // means `cur_depth <= MAX_INLINE_DEPTH` and the
        // trace head's frame is still live.
        let depth_u8 = cur_depth as u8;
        if depth_u8 > self.jit.max_depth_seen {
            self.jit.max_depth_seen = depth_u8;
        }
        // Fix up a prior `Op::Call C=0` (multi-
        // return / variable return count). Recorder pushed
        // it with var_count=None before the call dispatched;
        // now that the call has returned and we're about to
        // push the next op, top reflects the actual return
        // count. Snapshot top - (caller.base + call.a).
        if let Some(last) = rec.ops.last_mut()
            && matches!(last.inst.op(), crate::vm::isa::Op::Call)
            && last.inst.c() == 0
            && last.var_count.is_none()
            && let Some(f) = self.frames.last().and_then(CallFrame::lua)
        {
            let from = f.base + last.inst.a();
            if self.top >= from {
                last.var_count = Some(self.top - from);
            }
        }
        // For SetList B=0, snapshot the source
        // count = top - A - 1 (mirrors Lua's `n = top - ra
        // - 1` from lvm.c OP_SETLIST). Sources are
        // R[A+1..top), exclusive top. For Call C=0's
        // var_count (the return count = top - A inclusive),
        // see the prior-op fix-up above; here we
        // initialise the current Call op to None and let
        // the fix-up on the next op's push populate it.
        let var_count = if matches!(inst.op(), crate::vm::isa::Op::SetList)
            && inst.b() == 0
            && let Some(f) = self.frames.last().and_then(CallFrame::lua)
        {
            let from = f.base + inst.a();
            if self.top > from {
                Some(self.top - from - 1)
            } else {
                None
            }
        } else {
            None
        };
        let op = crate::jit::trace::RecordedOp {
            proto: cl.proto,
            pc,
            inst,
            inline_depth: depth_u8,
            var_count,
        };
        // Depth>0 Return0/Return1 mirrors
        // LuaJIT's `IR_RETF` (lj_record.c:922+ lj_record_ret).
        // Captured as a side-channel `RetfRecord` parallel to
        // `ops` when `self_link_enabled` is on. The
        // down-rec stitch consumes these to guard side-trace
        // inlined-frame topology against the recorded shape.
        // Gated on the same flag as the cycle catch so the
        // ship-default path (p16 off) sees zero behavior
        // change. `caller_pc` is the recorded enclosing Call's
        // pc + 1 — interp's resume point after the inlined
        // frame pops.
        if self.jit.self_link_enabled
            && depth_u8 > 0
            && matches!(
                inst.op(),
                crate::vm::isa::Op::Return0 | crate::vm::isa::Op::Return1
            )
        {
            self.trace_record_retf(inst, pc, depth_u8);
        }
        let field_ic = self.jit.field_ic_enabled;
        let rec = self.jit.active_trace.as_mut().expect("recording");
        // Capture FieldIcSnapshot for the
        // FIRST eligible Op::GetField site when the Vm's field IC
        // switch is on. "Eligible" means:
        //   - R[B] is Value::Table with metatable.is_none()
        //   - K[C] is Value::Str
        //   - The string key actually occupies a hash slot
        //     (so the IC's slot_idx is a real index, not
        //     a probe sentinel).
        // Once captured, subsequent GetFields skip this
        // logic (rec.field_ic_snapshot.is_some() short-
        // circuits).
        if field_ic
            && rec.field_ic_snapshot.is_none()
            && matches!(inst.op(), crate::vm::isa::Op::GetField)
        {
            let b = inst.b();
            let c_idx = inst.c() as usize;
            let r_b = self.stack[(base + b) as usize];
            if let Value::Table(g) = r_b
                && g.metatable().is_none()
                && c_idx < cl.proto.consts.len()
                && let Value::Str(s) = cl.proto.consts[c_idx]
            {
                let key = Value::Str(s);
                let tbl_ref = &*g;
                if let Some(slot_idx) = tbl_ref.find_node_idx(key)
                    && let Some(val) = tbl_ref.node_val_at(slot_idx)
                {
                    let op_idx = rec.ops.len() as u32;
                    rec.field_ic_snapshot = Some(crate::jit::trace_types::FieldIcSnapshot {
                        op_idx,
                        nodes_len: tbl_ref.nodes_capacity() as u64,
                        slot_idx: slot_idx as u64,
                        key_ptr_bits: s.as_ptr() as u64,
                        cached_val_tag: val.tag_byte(),
                    });
                    self.jit.counters.field_ic_snapshot_captured += 1;
                }
            }
        }
        let slot = self.field_slot_of(cl, inst, base);
        let rec = self.jit.active_trace.as_mut().expect("recording");
        if !rec.push(op) {
            // recorder overflow (MAX_TRACE_LEN)
            self.abort_recording("trace-overflow");
        } else if let (Some(slot), Some(s)) = (slot, rec.field_slots.last_mut()) {
            *s = slot;
        }
    }

    /// For `GetField` / `SetField` / `Self`, the hash slot of the table
    /// operand holding the constant string key, if the key is there.
    fn field_slot_of(&self, cl: Gc<LuaClosure>, inst: Inst, base: u32) -> Option<u32> {
        use crate::vm::isa::Op;
        let (t, k) = match inst.op() {
            Op::GetField | Op::SelfOp => (inst.b(), inst.c()),
            Op::SetField => (inst.a(), inst.b()),
            _ => return None,
        };
        let Value::Table(t) = *self.stack.get((base + t) as usize)? else {
            return None;
        };
        let key @ Value::Str(_) = *cl.proto.consts.get(k as usize)? else {
            return None;
        };
        t.find_node_idx(key).map(|i| i as u32)
    }

    /// Drop the recording, tallied under `cause`. Counted like a failed
    /// compile: a head whose recordings keep aborting is given up instead
    /// of being recorded again on every hot crossing.
    fn abort_recording(&mut self, cause: &'static str) {
        let rec = self.jit.active_trace.take().expect("recording");
        self.jit.counters.aborted += 1;
        self.jit.counters.bump_close_cause(cause);
        note_trace_compile_failure(rec.head_proto, rec.head_pc);
    }

    /// True when `inst` is arithmetic that can take an integer operand.
    fn int_operand(&self, inst: Inst, base: u32) -> bool {
        use crate::vm::isa::Op;
        let is_int = |r: u32| matches!(self.stack[(base + r) as usize], Value::Int(_));
        match inst.op() {
            Op::Add | Op::Sub | Op::Mul | Op::Mod => is_int(inst.b()) || is_int(inst.c()),
            Op::Unm | Op::AddI | Op::SubI | Op::AddK | Op::SubK | Op::MulK | Op::ModK => {
                is_int(inst.b())
            }
            _ => false,
        }
    }

    /// A depth>0 `Return0` / `Return1` during recording (LuaJIT `IR_RETF`):
    /// note the return for the down-recursion stitch.
    fn trace_record_retf(&mut self, inst: Inst, pc: u32, depth_u8: u8) {
        let rec = self.jit.active_trace.as_mut().expect("recording");
        let results: u8 = match inst.op() {
            crate::vm::isa::Op::Return0 => 0,
            crate::vm::isa::Op::Return1 => 1,
            _ => 0,
        };
        // Most recent Op::Call recorded at the caller's
        // depth (`depth_u8 - 1`) is the frame this Return
        // is unwinding from. Reverse scan stops at the
        // first match.
        let caller_depth = depth_u8 - 1;
        let caller_call = rec.ops.iter().rev().find(|r| {
            r.inline_depth == caller_depth && matches!(r.inst.op(), crate::vm::isa::Op::Call)
        });
        let caller_pc = caller_call.map(|r| r.pc + 1).unwrap_or(pc);
        // Capture the caller's proto
        // for the RetfRecord. LuaJIT `IR_RETF.op1`
        // equivalent. For fib(28) the caller's proto
        // equals the trace head; for future mutual
        // recursion the recorded Op::Call's proto is the
        // right target. Fallback to head_proto when no
        // enclosing Call op was captured (mirrors
        // `caller_pc`'s fallback to the Return's own pc).
        let caller_proto = caller_call.map(|r| r.proto).unwrap_or(rec.head_proto);
        rec.retfs.push(crate::jit::trace::RetfRecord {
            from_depth: depth_u8,
            to_depth: caller_depth,
            results,
            caller_pc,
            proto: caller_proto,
        });
        // DownRec close trigger:
        // count RetfRecords on this recording whose
        // `proto` matches `caller_proto` (LuaJIT
        // `check_downrec_unroll` chain filter
        // `op1 == ptref`). Threshold mirrors
        // RECUNROLL_THRESHOLD; first trip stamps the
        // `downrec_close` marker, subsequent retfs
        // keep the marker without overwrite. The
        // lowerer's end_idx picker routes through
        // TraceEnd::DownRec when the marker is set.
        if rec.downrec_close.is_none() {
            let caller_proto_ptr = caller_proto.as_ptr();
            let prior_match_count = rec
                .retfs
                .iter()
                .filter(|r| r.proto.as_ptr() == caller_proto_ptr)
                .count();
            // Strictly-greater-than threshold matches
            // LuaJIT `count + J->tailcalled > recunroll`.
            // The newly-pushed retf is already counted.
            if prior_match_count > crate::jit::trace::RECUNROLL_THRESHOLD {
                rec.downrec_close = Some(crate::jit::trace::DownRecClose {
                    return_pc: caller_pc,
                    target_proto: caller_proto,
                    depth_delta: 1,
                });
                // Close-cause taxonomy: tag the
                // restart with `"downrec-restart"`. The
                // lowerer adds `"downrec-stitch-failed"` when
                // the lifted back-edge falls back to
                // deopt.
                self.jit.counters.bump_close_cause("downrec-restart");
            }
        }
    }
}
