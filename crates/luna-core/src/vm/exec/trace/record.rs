//! Trace recording in the dispatch loop: the instruction about to run is
//! appended to the active recording, which closes once the loop comes
//! back to its head.

use super::*;

mod retf;
mod self_link;
pub(in crate::vm::exec) mod slots;

impl Vm {
    /// Record `inst` (at `pc` of `cl`, frame `base`) into the active trace,
    /// or close the trace when the loop has returned to its head.
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_record_step(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        inst: Inst,
        base: u32,
    ) {
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
        let rec = self.jit.active_trace.as_ref().expect("recording");
        let at_head_loop = cur_depth == 0
            && !rec.ops.is_empty()
            && !returned_past_head
            && std::ptr::eq(cl.proto.as_ptr(), rec.head_proto.as_ptr())
            && pc == rec.head_pc;
        if !returned_past_head {
            // past a return out of the head frame, the host calling the
            // function again at the head closes the recording there; any
            // other op is another call's
            if rec.returned_from_head && !at_head_loop {
                self.abort_recording("left-start-frame");
                return;
            }
            self.note_result_tag(cl, base, cur_depth);
        }
        let self_link_trip = self.self_link_trip(cl, pc, cur_depth, returned_past_head);
        if let Some(kind) = self_link_trip {
            self.relax_self_link(kind, cur_depth);
        }
        let should_close =
            at_head_loop || returned_past_head || depth_cap_hit || self_link_trip.is_some();
        if should_close {
            if at_head_loop && let Some(rec) = self.jit.active_trace.as_mut() {
                rec.closed_at_head = true;
            }
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
        let rec = self.jit.active_trace.as_mut().expect("recording");
        // Depth-aware push at the
        // current `cur_depth`. The `depth_cap_hit` /
        // `returned_past_head` early-exit is handled by
        // the `should_close` branch above; reaching here
        // means `cur_depth <= MAX_INLINE_DEPTH` and the
        // trace head's frame is still live.
        let depth_u8 = cur_depth as u8;
        rec.returned_from_head |= depth_u8 == 0
            && matches!(
                inst.op(),
                crate::vm::isa::Op::Return
                    | crate::vm::isa::Op::Return0
                    | crate::vm::isa::Op::Return1
            );
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
        let index = match slot {
            None => self.index_slots_of(cl, inst, base),
            Some(_) => None,
        };
        let index_key = self.mm_names[Mm::Index as usize];
        self.note_for_step(inst, base, cur_depth);
        let rec = self.jit.active_trace.as_mut().expect("recording");
        if !rec.push(op) {
            // recorder overflow (MAX_TRACE_LEN)
            self.abort_recording("trace-overflow");
        } else {
            if let (Some(slot), Some(s)) = (slot, rec.field_slots.last_mut()) {
                *s = slot;
            }
            if let (Some((m, k)), Some(s)) = (index, rec.index_slots.last_mut()) {
                *s = u64::from(m) << 32 | u64::from(k);
                rec.index_key = Some(index_key);
            }
        }
    }

    /// Drop the recording, tallied under `cause`. Counted like a failed
    /// compile: a head whose recordings keep aborting is given up instead
    /// of being recorded again on every hot crossing.
    pub(in crate::vm::exec) fn abort_recording(&mut self, cause: &'static str) {
        let rec = self.jit.active_trace.take().expect("recording");
        self.jit.counters.aborted += 1;
        self.jit.counters.bump_close_cause(cause);
        note_trace_compile_failure(&mut self.jit.counters, rec.head_proto, rec.head_pc);
    }
}
