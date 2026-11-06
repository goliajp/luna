//! A return out of an inlined frame during recording, noted for the
//! down-recursion stitch.

use super::*;

impl Vm {
    /// A depth>0 `Return0` / `Return1` during recording (LuaJIT `IR_RETF`):
    /// note the return for the down-recursion stitch.
    pub(super) fn trace_record_retf(&mut self, inst: Inst, pc: u32, depth_u8: u8) {
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
