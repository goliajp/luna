//! The self-link and down-recursion close shapes a recording can end in.

use super::*;

/// Distinguishes the two self-link close shapes. UpRec
/// corresponds to LJ's `LJ_TRLINK_UPREC` (fib's case — recursion is
/// non-tail, framedepth > 0 at close). TailRec corresponds to
/// `LJ_TRLINK_TAILREC` (factorial's tail-recursive form, depth == 0
/// at close — Lua bytecode rarely produces this without explicit TCO
/// support, but the variant is kept symmetric with LJ's enum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfRecKind {
    /// Tail-recursive self-link (LuaJIT `LJ_TRLINK_TAILREC`).
    TailRec,
    /// Up-recursive self-link (LuaJIT `LJ_TRLINK_UPREC`).
    UpRec,
}

/// A recorded return-from-inlined-frame event,
/// captured by the recorder when a depth>0 `Op::Return0` /
/// `Op::Return1` fires during active recording with
/// `self_link_enabled = true`. Mirrors LuaJIT's `IR_RETF`
/// (`lj_ir.h`) — the IR-level marker that the inlined call frame at
/// `from_depth` returned to `to_depth` with `results` values.
///
/// The records form a side-channel parallel to [`TraceRecord::ops`].
/// The down-rec stitch reads them to verify that a side-trace's
/// inlined-frame topology matches the recorded shape before stitching.
///
/// `caller_pc` is the PC the inlined frame returns TO in its caller
/// (`enclosing_call.pc + 1`), captured at record-time so the
/// stitch can guard equality against the runtime caller PC.
///
/// `proto: Gc<Proto>` is the proto
/// the inlined frame is returning *to* (the caller's proto). Mirrors
/// LuaJIT `IR_RETF.op1 = ir_kgc(IR(ptref))` which carries the target
/// proto pointer (see `lj_record.c:897 check_downrec_unroll` chain
/// filter `op1 == ptref`). For luna's fib(28) self-recursion this
/// equals `TraceRecord.head_proto`, but the field stays explicit
/// so future mutual-recursion (`fib_even` ↔ `fib_odd`) closes through
/// the same code path without a head_proto identity assumption.
/// GC: `RetfRecord.proto` follows the same transitive-reachability
/// reasoning as `RecordedOp.proto` — not explicitly enrolled in
/// `Vm::gc_roots()` because the captured proto is, at record time,
/// reachable via a running `CallFrame`'s closure.
///
/// No `PartialEq, Eq` derives because `Gc<T>` intentionally doesn't
/// impl those traits (use `Gc::ptr_eq` for pointer-identity equality).
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct RetfRecord {
    /// Depth this return originated from (>0; the frame about to be
    /// popped).
    pub from_depth: u8,
    /// Depth control returns to (`from_depth - 1`).
    pub to_depth: u8,
    /// Number of return values (`0` for `Op::Return0`, `1` for
    /// `Op::Return1`). Variadic `Op::Return` isn't recorded at
    /// depth>0 today.
    pub results: u8,
    /// PC the caller resumes at after the inlined frame pops
    /// (`enclosing_call.pc + 1`). Used by the stitch to guard return-target
    /// equality at runtime.
    pub caller_pc: u32,
    /// The caller's proto (target of the return).
    /// LuaJIT `IR_RETF.op1` equivalent. The lowerer reads this to
    /// emit a proto-identity assertion at the stitch guard; the
    /// dispatcher consults it when materialising CallFrames for
    /// stitched re-entry. For fib(28) self-recursion it equals
    /// `TraceRecord.head_proto`; kept explicit for forward-compat
    /// with mutual-recursion patterns.
    pub proto: Gc<Proto>,
}

/// Recorder-side close marker for the down-rec
/// stitched-side-trace shape. Set by the recorder when a depth>0
/// `Op::Return` fires inside an active recording AND the prior
/// `rec.retfs` chain shows the trace is bouncing in-and-out of a
/// single proto past [`RECUNROLL_THRESHOLD`] (the LuaJIT
/// `lj_record.c:912 lj_trace_err(LJ_TRERR_DOWNREC)` trigger
/// condition). The lowerer's `end_idx` picker reads this BEFORE the
/// `self_link_kind` arm and routes through the new
/// `TraceEnd::DownRec` close. The lowerer reads
/// `DownRecClose.target_proto` + `return_pc` and emits the
/// `asm_retf`-equivalent guard sequence; the dispatcher follows the
/// stitch.
#[derive(Clone, Copy, Debug)]
#[doc(hidden)]
pub struct DownRecClose {
    /// PC the inlined-frame `Return` is unwinding to. Used by the
    /// lowerer to bake the guard-target into the stitch IR and by
    /// the dispatcher to resume interp at the correct caller PC
    /// on stitch-miss.
    pub return_pc: u32,
    /// Caller proto the down-rec is unwinding to — mirrors LuaJIT
    /// `LJ_TRLINK_DOWNREC` parent-proto association. For fib(28)
    /// self-recursion this equals `TraceRecord.head_proto`; the
    /// field is kept explicit so the close marker matches the
    /// shape the guard predicate consumes.
    pub target_proto: Gc<Proto>,
    /// Depth delta the close marker observed — `from_depth - to_depth`
    /// at the moment the recorder tripped the catch. Always `1` for
    /// today's down-rec catch (depth>0 → depth-1 Return); kept as a
    /// u8 so diag rows can surface non-`1` values when future
    /// multi-level unrolls are wired up.
    pub depth_delta: u8,
}
