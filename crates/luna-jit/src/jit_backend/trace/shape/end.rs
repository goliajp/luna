//! Where a trace ends, and how.

use super::super::*;

/// Which terminating op (if any) sits at the trace's effective
/// tail position. See the comment block in
/// [`try_compile_trace_with_options`] for the contracts on each.
#[derive(Clone, Copy, Debug)]
pub(in super::super) enum TraceEnd {
    Call,
    ForLoop,
    /// the trace's inline-recursion path hit something
    /// the lowerer can't continue past (ForLoop@d>0, a non-self
    /// Call@d>0, depth past MAX_INLINE_DEPTH, or a proto mismatch).
    /// emit `ops[..i]` normally, then close the tail with a
    /// store-back + return of `record.ops[i].pc`. Dispatchable is
    /// forced false because the interp can't resume at that PC
    /// without first materialising the depth>0 CallFrames. cmp@d>0
    /// does not land here: it emits a real side-exit via the
    /// frame-mat helper.
    InlineAbort,
    /// `Op::Return0` / `Op::Return1` at depth=0
    /// terminates the trace (the caller frame unwinds). Treat as a
    /// truncation point: emit `ops[..i]` normally, then store back
    /// the caller window + return `record.ops[i].pc`. The interp
    /// re-executes the Return instruction with the correct PC. Same
    /// shape as `TraceEnd::Call` but emitted by a different op, so
    /// kept as a separate variant for the tail dispatch.
    Return,
    /// Recorder detected self-recursion via the cycle catch
    /// (same-proto ancestor count > [`RECUNROLL_THRESHOLD`] at the
    /// head_pc on head_proto). The trace body covers the inlined
    /// recursion levels, but the lowerer's tail is a plain deopt
    /// (store back the caller window, return `head_pc`) and the trace
    /// is not dispatchable: restoring a snapshot across the back-edge
    /// gives wrong results for non-tail self-recursion such as fib.
    SelfLink(SelfRecKind),
    /// recorder detected a down-recursion close
    /// shape: a depth>0 `Op::Return` fired during recording AND the
    /// `rec.retfs` chain showed the trace bouncing in-and-out of the
    /// same caller-proto past [`RECUNROLL_THRESHOLD`]. Mirrors
    /// LuaJIT's `LJ_TRLINK_DOWNREC` close (`lj_record.c:912
    /// lj_trace_err(LJ_TRERR_DOWNREC)` → `lj_trace.c:570
    /// trace_downrec` → restart-at-Return-PC). Routed BEFORE the
    /// `SelfLink` arm in the `end_idx` picker so depth>0 Return
    /// closes win over depth>0 self-link cycles.
    ///
    /// `return_pc` is the PC the inlined frame is unwinding to —
    /// the stitch-entry head_pc the lowerer bakes into the
    /// retf-guard sequence (`asm_retf` equivalent).
    /// `target_proto_id` carries the target proto's `Gc::as_ptr()`
    /// as a raw `usize` so this enum stays `Copy` for the existing
    /// `end_idx_opt: Option<(usize, TraceEnd)>` plumbing. The
    /// matching `Gc<Proto>` lives on `TraceRecord.downrec_close.
    /// target_proto` (not erased); the lowerer cross-references the
    /// two when emitting the guard.
    ///
    /// The lowerer emits the retf-guard + stitch sentinel for this
    /// arm, falling back to the safe deopt-tail on a guard miss.
    DownRec {
        /// PC the Return is unwinding to (caller's resume PC).
        return_pc: u32,
        /// Caller proto's `Gc::as_ptr()` as `usize` (kept opaque so
        /// `TraceEnd` stays `Copy`). The lowerer reads
        /// `TraceRecord.downrec_close.target_proto` for the real
        /// `Gc<Proto>`.
        target_proto_id: usize,
        /// `from_depth - to_depth` at the moment the catch tripped.
        /// Always `1` today.
        depth_delta: u8,
    },
}

/// Where the plain terminator scan ends a trace: the first depth-0
/// `Call` that is not self-recursive, `ForLoop` / `TForLoop`, depth-0
/// return, or an op the inline path cannot hold (`InlineAbort`).
/// Self-recursive calls are walked past; the callee's ops follow at
/// depth + 1. `None` when the whole record is body.
pub(in super::super) fn plain_trace_end(
    record: &TraceRecord,
    folded_ops: &[bool],
    calls: &[Option<InlineCall>],
) -> Option<(usize, TraceEnd)> {
    let mut found: Option<(usize, TraceEnd)> = None;
    for (i, r) in record.ops.iter().enumerate() {
        if folded_ops[i] {
            continue;
        }
        let depth = r.inline_depth as usize;
        if depth > MAX_INLINE_DEPTH as usize {
            found = Some((i, TraceEnd::InlineAbort));
            break;
        }
        match r.inst.op() {
            Op::Call => {
                if call_inlinable(record, calls, i) {
                    // Continue walking — Op::Call emits nothing in
                    // the inline path and op_offsets handles the
                    // window shift for the callee's subsequent ops.
                    continue;
                }
                if depth == 0 {
                    found = Some((i, TraceEnd::Call));
                } else {
                    found = Some((i, TraceEnd::InlineAbort));
                }
                break;
            }
            // a side trace linked to the trace it reached goes on through
            // its own frame's loop edges
            Op::ForLoop | Op::ForLoop55 | Op::ForLoopF | Op::ForLoop55F
                if depth == 0 && record.link_pc.is_some() => {}
            Op::ForLoop | Op::ForLoop55 | Op::ForLoopF | Op::ForLoop55F => {
                if depth == 0 {
                    found = Some((i, TraceEnd::ForLoop));
                } else {
                    found = Some((i, TraceEnd::InlineAbort));
                }
                break;
            }
            // generic-for back-edge. Same tail
            // emit slot as Op::ForLoop (TraceEnd::ForLoop); the
            // tail emit branches on `record.ops[idx].inst.op()`
            // to pick the right side-exit predicate (count>0 vs
            // R[A+4] tag check).
            Op::TForLoop | Op::TForLoop53 | Op::TForLoop55 => {
                if depth == 0 {
                    found = Some((i, TraceEnd::ForLoop));
                } else {
                    found = Some((i, TraceEnd::InlineAbort));
                }
                break;
            }
            Op::Return0 | Op::Return1 if depth == 0 => {
                found = Some((i, TraceEnd::Return));
                break;
            }
            // depth>0 Returns are inline-path unwinds; the
            // body emit loop handles them (Return0 no-op,
            // Return1 copy-back). Don't terminate.
            _ => {}
        }
    }
    found
}
