//! Closing a recording that re-enters its head from inside an inlined
//! recursion: as a self link, or as the down-recursion close it relaxes to.

use super::*;

impl Vm {
    /// Whether recording is about to re-enter the trace head from inside an
    /// inlined recursion level often enough to close as a self link.
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
    pub(super) fn self_link_trip(
        &self,
        cl: Gc<LuaClosure>,
        pc: u32,
        cur_depth: usize,
        returned_past_head: bool,
    ) -> Option<crate::jit::trace::SelfRecKind> {
        let rec = self.jit.active_trace.as_ref().expect("recording");
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
    }

    /// The self link the recording closes with (`kind`), or the down-
    /// recursion close it relaxes to at depth 2 and more.
    pub(super) fn relax_self_link(
        &mut self,
        kind: crate::jit::trace::SelfRecKind,
        cur_depth: usize,
    ) {
        let rec = self.jit.active_trace.as_mut().expect("recording");
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
                r.inline_depth == caller_depth_u8 && matches!(r.inst.op(), crate::vm::isa::Op::Call)
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
}
