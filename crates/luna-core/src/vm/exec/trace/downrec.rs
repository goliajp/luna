//! Down-recursion traces: classifying a return through their close.

use super::*;

impl Vm {
    /// The downrec trace returned through its close: count the stitch hit or
    /// miss, block the next downrec admit, and pop frames the trace pushed.
    pub(super) fn trace_downrec_classify(&mut self, continuation_pc: i64, pre_frames: usize) {
        // Downrec event classifier.
        let raw_ret = continuation_pc as u64;
        let sentinel_code = ((raw_ret >> 56) & 0x7F) as u32;
        if crate::jit::trace_types::is_downrec_sentinel(sentinel_code) {
            // Guard HIT — saved_pc matched one of the
            // baked candidates and the trace's
            // `stitch_blk` arm returned the DOWNREC
            // sentinel. Cycle-safety checkpoint:
            // decrement budget; on underflow,
            // reclassify as deopt + reset budget.
            // `STITCH_DEPTH_DEFAULT = 32` lets
            // ~all natural HITs in a hot loop fire
            // before reset pressure.
            if self.jit.stitch_depth_remaining > 0 {
                self.jit.stitch_depth_remaining -= 1;
                self.jit.counters.downrec_dispatched += 1;
            } else {
                self.jit.counters.downrec_deopt += 1;
                self.jit.stitch_depth_remaining =
                    crate::vm::jit_state::JitState::STITCH_DEPTH_DEFAULT;
            }
        } else {
            // Guard MISS via the lowerer's deopt_blk
            // arm (GLOBAL sentinel + body == head_pc).
            // The deopt_blk emit performs the
            // store-back via `emit_store_back_and_return_pc`,
            // so the live stack already reflects the
            // body's writes; no extra restore needed
            // from the dispatcher side.
            self.jit.counters.downrec_deopt += 1;
        }
        self.jit.suppress_downrec_admit_once = true;
        // Pop helper-pushed inlined frames (defensive —
        // the downrec emit shape doesn't push frames in the
        // tail, but a body side-exit before reaching
        // the tail may have via the materialize helper).
        while self.frames.len() > pre_frames {
            self.pop_frame();
        }
    }

    /// The caller's pc a downrec trace guards on.
    // Saved-PC slot population. The
    // recorded `dr_return_pc` on the closing trace is
    // the caller's resume PC captured at a depth>0
    // Return push (recorder push site). The natural runtime analogue for self-
    // stitch is the dispatching frame's PARENT frame's
    // PC: the trace's head_pc sits inside a Lua frame,
    // and the parent (caller) frame's `pc` is what
    // luna would observe as `[base-8]` in the LJ
    // `asm_retf` shape (`lj_asm_arm64.h:565`). When
    // the parent isn't a Lua frame (top-level dispatch
    // — first invocation through `call_value`), no
    // saved PC exists; we write 0, which always
    // mismatches the recorded `dr_return_pc != 0`
    // invariant (debug-asserted in the luna-jit trace
    // lowerer).
    pub(super) fn downrec_saved_pc(&self, pre_frames: usize) -> i64 {
        if pre_frames >= 2 {
            match &self.frames[pre_frames - 2] {
                CallFrame::Lua(parent) => parent.pc as i64,
                CallFrame::Cont(_) => 0,
            }
        } else {
            0
        }
    }
}

/// Whether a downrec trace's return is its close: the stitch_blk DOWNREC
/// sentinel (hit) or the deopt_blk GLOBAL sentinel with body == head_pc (miss).
pub(super) fn downrec_close_exit(continuation_pc: i64, head_pc_val: u32) -> bool {
    // Only enter the
    // downrec classifier for returns whose shape
    // matches the lowerer's `downrec_idx_opt` tail
    // emit: either the stitch_blk DOWNREC sentinel
    // (HIT) or the deopt_blk GLOBAL-sentinel-with-
    // body==head_pc (MISS via guard fail). Any
    // other return from a downrec trace (intermediate
    // body cmp side-exit, GetField inference fail,
    // etc.) carries a different sentinel/body shape
    // and means the body exited BEFORE reaching the
    // downrec close — classify those through the
    // normal decode path (else branch below) so
    // reg_state restores + pc advances correctly.
    // Classifying them all as MISS would skip the
    // normal restore, inflating `downrec_deopt` with
    // non-downrec events and losing the trace's
    // mid-flight writes.
    let raw_ret = continuation_pc as u64;
    let from_side_trace = (raw_ret >> 63) & 1 == 1;
    let sentinel_code = if from_side_trace {
        ((raw_ret >> 56) & 0x7F) as u32
    } else {
        0
    };
    let raw_body = raw_ret & 0x00FF_FFFF_FFFF_FFFFu64;
    let global_deopt_code = crate::jit::trace_types::encode_side_sentinel(
        crate::jit::trace_types::SIDE_SENT_KIND_GLOBAL,
        0,
    );
    from_side_trace
        && (crate::jit::trace_types::is_downrec_sentinel(sentinel_code)
            || (sentinel_code == global_deopt_code && raw_body == head_pc_val as u64))
}
