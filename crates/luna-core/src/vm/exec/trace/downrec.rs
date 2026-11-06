//! Down-recursion traces: classifying a return through their close.

use super::*;

impl Vm {
    /// The downrec trace returned through its close's stitch: count the hit
    /// (or, once the stitch budget is spent, a miss), block the next downrec
    /// admit, and pop frames the trace pushed.
    pub(super) fn trace_downrec_classify(&mut self, pre_frames: usize) {
        // Cycle-safety checkpoint: decrement budget; on underflow,
        // reclassify as deopt + reset budget. `STITCH_DEPTH_DEFAULT = 32`
        // lets ~all natural HITs in a hot loop fire before reset pressure.
        if self.jit.stitch_depth_remaining > 0 {
            self.jit.stitch_depth_remaining -= 1;
            self.jit.counters.downrec_dispatched += 1;
        } else {
            self.jit.counters.downrec_deopt += 1;
            self.jit.stitch_depth_remaining = crate::vm::jit_state::JitState::STITCH_DEPTH_DEFAULT;
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

/// Whether a downrec trace's return is its close's stitch: the stitch_blk
/// DOWNREC sentinel (hit). A miss leaves through the deopt_blk, which
/// stores the registers back and returns the head pc as a plain exit.
pub(super) fn downrec_close_exit(continuation_pc: i64) -> bool {
    let raw_ret = continuation_pc as u64;
    (raw_ret >> 63) & 1 == 1
        && crate::jit::trace_types::exit_code::is_downrec_sentinel(((raw_ret >> 56) & 0x7F) as u32)
}
