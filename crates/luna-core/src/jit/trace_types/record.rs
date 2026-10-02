//! The recorder's view of a trace: recorded ops and the record being built.

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

/// A single bytecode op as captured during trace recording, with the
/// runtime context needed to emit cranelift guards (register kinds,
/// metatable null checks, etc.). Stored in `TraceRecord.ops`.
#[derive(Clone, Debug)]
pub struct RecordedOp {
    /// Original Proto + PC that produced this op. Multiple
    /// `RecordedOp`s with different `proto` come from inlined calls.
    pub proto: Gc<Proto>,
    /// Pc within `proto` at which this op was recorded.
    pub pc: u32,
    /// The bytecode instruction itself (copy — Proto.code is
    /// already immutable post-compile).
    pub inst: Inst,
    /// Depth of inlined recursion above the trace head. 0 = the
    /// outer trace; positive values come from inlining.
    pub inline_depth: u8,
    /// Recorder snapshot of the runtime variable count
    /// for ops whose B / C field is `0` (meaning "use stack top").
    /// - `Op::Call` with `C == 0`: snapshot of `top - A` AFTER the
    ///   call returns — i.e. the actual number of values the
    ///   callee returned this trip.
    /// - `Op::SetList` with `B == 0`: snapshot of `top - A` at the
    ///   op — i.e. the number of source slots `[A+1..top]`.
    /// - All other ops: `None`.
    /// Emit consumes this as a compile-time constant guarded by a
    /// runtime equality check.
    pub var_count: Option<u32>,
}

/// `LUNA_JIT_FIELD_IC` env gate.
///
/// Default OFF (env unset or set to anything other than `1` / `true`).
/// When ON, the trace recorder captures a [`FieldIcSnapshot`] at the
/// first eligible `Op::GetField` site and the trace lowerer replaces
/// the helper call at that site with an inline cache: 4 guards
/// (`mt is None`, `nodes.len() == cached`, `nodes[slot].key ==
/// cached_key_bits`, `val.tag == cached_tag`) + 1 load of the value's
/// raw payload. Guard miss falls through to the existing helper
/// (scaffold-safe, no new deopt edges).
///
/// Read once per process and cached. It is only the default of each new
/// Vm's switch (`Vm::set_field_ic_enabled`): the recorder checks the
/// Vm's switch, and the lowerer emits the cache wherever the record
/// carries a snapshot.
pub fn field_ic_enabled() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0 = uninitialised, 1 = off, 2 = on. Sentinel encoding lets a
    // single relaxed load distinguish "decision cached" from "ask
    // env" without a Mutex.
    static CACHED: AtomicU8 = AtomicU8::new(0);
    let v = CACHED.load(Ordering::Relaxed);
    if v != 0 {
        return v == 2;
    }
    let enabled = std::env::var("LUNA_JIT_FIELD_IC")
        .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    CACHED.store(if enabled { 2 } else { 1 }, Ordering::Relaxed);
    enabled
}

/// Table-field IC snapshot captured by the recorder
/// at the **first** eligible `Op::GetField` site in the trace, when the
/// recording Vm has its field IC switch on.
///
/// "Eligible" means the receiver `R[B]` is `Value::Table` with no
/// metatable at recorder-fire time AND the key resolves to a
/// `Value::Str` in `head_proto.consts[C]` AND the key actually
/// occupies a hash slot at recording time. The recorder bakes the
/// cached `(nodes_len, slot_idx, key_ptr_bits, val_tag)` tuple here so
/// the lowerer can emit guards against the table's live layout.
///
/// Only a single snapshot is supported (the first eligible site).
#[derive(Clone, Copy, Debug)]
pub struct FieldIcSnapshot {
    /// Index into `TraceRecord.ops` of the `Op::GetField` this
    /// snapshot describes. The lowerer matches `op_idx == i` at
    /// emit time to decide whether to fire the IC path or fall
    /// through to the original helper-call path.
    pub op_idx: u32,
    /// The table's hash-part node count at recorder-fire time. The IC's
    /// shape guard (load the node mask, compare with this count less one)
    /// bails to the helper on mismatch so a rehash deopts predictably.
    pub nodes_len: u64,
    /// Index of the `Node` slot that holds the cached key. The IC
    /// computes `node_addr = nodes_ptr + slot_idx * SIZEOF_NODE`
    /// and reads `key`/`val` from that address.
    pub slot_idx: u64,
    /// `Gc<LuaStr>` raw pointer bits for the cached key. The IC's
    /// slot-key guard (`load i64, icmp Equal, cached_key_bits`)
    /// catches the case where a rehash or insert relocated a
    /// different key into the cached slot.
    pub key_ptr_bits: u64,
    /// Recorder-time tag byte of the slot's value. The IC's val
    /// guard (`load i8, icmp Equal, cached_val_tag`) catches the
    /// case where a `SetField` mutated the slot to a different
    /// type since recording — without this guard the lowerer's
    /// downstream `current_kinds` propagation could pack stale
    /// raw bits as the wrong tag → garbage Value on the next op.
    pub cached_val_tag: u8,
}

/// A recorded trace: a linear sequence of ops starting at a back-edge
/// target PC, terminating at either a loop close (back to head) or a
/// hard exit (return, error).
#[derive(Clone, Debug)]
pub struct TraceRecord {
    /// The PC the trace starts at (back-edge target).
    pub head_proto: Gc<Proto>,
    /// Pc within `head_proto` where the trace begins (the back-edge target).
    pub head_pc: u32,
    /// Per-register `Value` tag (from `runtime::value::raw`) at
    /// the moment recording started. Lengths matches the
    /// `head_proto.max_stack` window. Lowerer uses these to
    /// initialise per-reg kinds — a slot tagged FLOAT at entry
    /// means a subsequent `Add` op reading that reg lowers to
    /// `fadd` instead of `iadd`. Empty when the trace was built
    /// from a test harness that didn't snapshot.
    pub entry_tags: Vec<u8>,
    /// Ops in execution order.
    pub ops: Vec<RecordedOp>,
    /// `true` once the trace returns to `head_pc` (loop closes
    /// cleanly). `false` for fallthrough exits — those can still
    /// compile but never inline-loop.
    pub closed: bool,
    /// `true` if the recording was fired by a
    /// trace-on-call trigger (`begin_call`'s Lua callee arm), as
    /// opposed to a back-edge trigger (`Op::Jmp` neg / `Op::ForLoop`).
    /// Affects the dispatcher's close detection: call-triggered
    /// traces close on **any** re-entry of `(head_proto, head_pc)`
    /// (single-pass through the function body), while loop-triggered
    /// traces require `cur_depth == 0` so a nested call to the
    /// containing loop's function doesn't prematurely close.
    pub is_call_triggered: bool,
    /// Generic-for iter fn pointer snapshot.
    /// Populated by `Op::TForLoop`'s recorder trigger when
    /// `R[A]` is `Value::Native`. Lets the lowerer specialise
    /// `Op::TForCall` emit on `ipairs_iter` (inline Table aget
    /// via `TABLE_ARRAY_PTR_OFFSET` / `TABLE_ASIZE_OFFSET` —
    /// skip the `luna_jit_op_tforcall` C call entirely). `None`
    /// for non-generic-for traces or when the recorder fires
    /// for a non-Native iter.
    pub tfor_iter_ptr: Option<usize>,
    /// Snapshot of `R[A+5]` (the iter's value
    /// slot) tag at recorder fire. The ipairs inline aget emits a
    /// runtime guard `val_tag == expected_tag` (or Nil for the
    /// loop-end branch); a mismatch deopts to interp. Without the
    /// guard, mixed-tag arrays (e.g. `{'a', 1, 'c'}`) would let
    /// the Str-specialised spill pack non-Str raw bits as a Str
    /// pointer → garbage. `None` for non-generic-for traces or
    /// when the snapshot slot isn't reachable.
    pub tfor_val_tag: Option<u8>,
    /// If set, this trace is a SIDE TRACE: it was
    /// triggered by a parent trace's hot side-exit, NOT by the
    /// usual back-edge / call-trigger paths. The tuple is
    /// `(parent_head_proto, parent_head_pc, parent_exit_idx)`,
    /// uniquely identifying the parent's `CompiledTrace` and the
    /// `exit_hit_counts` slot that crossed [`HOTEXIT_THRESHOLD`].
    /// `None` for primary traces. Read to wire the parent's
    /// exit-branch indirection pointer to the side trace's entry
    /// once it compiles.
    pub side_trace_parent: Option<(Gc<Proto>, u32, usize)>,
    /// Set by the recorder cycle catch when a same-proto
    /// ancestor count exceeds [`RECUNROLL_THRESHOLD`] at head_pc on
    /// head_proto. Drives the lowerer's `TraceEnd::SelfLink` close
    /// shape (snapshot-restore + bump-base + branch-to-self), and
    /// inhibits `is_inline_abort_close` even though the recorded
    /// body has depth>0 ops. `None` for all non-self-link closes
    /// (Call truncation, ForLoop, Return, InlineAbort).
    pub self_link_kind: Option<SelfRecKind>,
    /// Side-channel of [`RetfRecord`]s captured
    /// when a depth>0 `Op::Return0` / `Op::Return1` fires during
    /// recording with `self_link_enabled = true`. Empty on the
    /// default path (p16 off). The records feed the down-rec stitch.
    pub retfs: Vec<RetfRecord>,
    /// Close marker set by the recorder when a
    /// depth>0 `Op::Return` re-trips the down-rec catch (i.e., the
    /// `rec.retfs` chain shows the current Return targets the same
    /// proto as a prior Retf AND the count of prior Retfs targeting
    /// that proto exceeds [`RECUNROLL_THRESHOLD`]). The lowerer's
    /// `end_idx` picker reads this BEFORE the `self_link_kind` arm
    /// and routes through `TraceEnd::DownRec`. `None` on the
    /// default path (p16 off) and on all non-down-rec closes.
    pub downrec_close: Option<DownRecClose>,
    /// Table-field IC snapshot for the first
    /// eligible `Op::GetField` site in the trace. Populated by the
    /// recorder when the Vm's field IC switch is on; `None` when it is
    /// off and on traces where no eligible site fires.
    pub field_ic_snapshot: Option<FieldIcSnapshot>,
    /// Per recorded op, the `raw` tag of the value it left in `R[A]` while
    /// it was recorded, or [`RESULT_TAG_UNKNOWN`] (an op that wrote no
    /// register, or ran into a call). The lowerer types a table read by
    /// it and checks the read against it.
    pub result_tags: Vec<u8>,
    /// Per recorded op, for a field read or write by a constant string
    /// key, the hash slot the key was found in while it was recorded, or
    /// [`FIELD_SLOT_UNKNOWN`]. The lowerer reads and writes that slot
    /// directly once it checks the slot still holds the key.
    pub field_slots: Vec<u32>,
}

/// [`TraceRecord::field_slots`] for an op with no slot.
pub const FIELD_SLOT_UNKNOWN: u32 = u32::MAX;

/// [`TraceRecord::result_tags`] for an op whose result was not seen.
pub const RESULT_TAG_UNKNOWN: u8 = u8::MAX;

impl TraceRecord {
    /// The tag op `i` was seen to leave in its `R[A]`, if any.
    pub fn result_tag(&self, i: usize) -> Option<u8> {
        self.result_tags
            .get(i)
            .copied()
            .filter(|&t| t != RESULT_TAG_UNKNOWN)
    }

    /// The hash slot op `i` found its key in, if any.
    pub fn field_slot(&self, i: usize) -> Option<u32> {
        self.field_slots
            .get(i)
            .copied()
            .filter(|&s| s != FIELD_SLOT_UNKNOWN)
    }

    /// Start a fresh recording at `head_pc` of `proto`. The
    /// `entry_tags` snapshot pins the per-slot `Value` tag at the
    /// moment recording fires; pass an empty vec for test
    /// harnesses that don't have a live stack to snapshot.
    /// `is_call_triggered = true` only when fired by a trace-on-call;
    /// back-edge triggers pass `false`.
    pub fn start(
        proto: Gc<Proto>,
        head_pc: u32,
        entry_tags: Vec<u8>,
        is_call_triggered: bool,
    ) -> Self {
        TraceRecord {
            head_proto: proto,
            head_pc,
            entry_tags,
            ops: Vec::with_capacity(MAX_TRACE_LEN),
            closed: false,
            is_call_triggered,
            tfor_iter_ptr: None,
            tfor_val_tag: None,
            side_trace_parent: None,
            self_link_kind: None,
            retfs: Vec::new(),
            downrec_close: None,
            field_ic_snapshot: None,
            result_tags: Vec::with_capacity(MAX_TRACE_LEN),
            field_slots: Vec::with_capacity(MAX_TRACE_LEN),
        }
    }

    /// Start a SIDE trace recording at a hot side-exit's
    /// `cont_pc`. The trace's head_proto is the proto interp resumed
    /// in after the side-exit fired (today: same as the parent's
    /// head_proto, since trace JIT only inlines self-recursive
    /// calls). `parent_*` identifies the parent `CompiledTrace`'s
    /// `exit_hit_counts` slot so the back-pointer can be wired.
    ///
    /// `is_call_triggered = false` for side traces — the close
    /// detection runs like a back-edge trigger (cur_depth==0 +
    /// pc==head_pc), and the discard heuristic for short
    /// call-triggered partials doesn't apply.
    pub fn start_side_trace(
        proto: Gc<Proto>,
        head_pc: u32,
        entry_tags: Vec<u8>,
        parent_head_proto: Gc<Proto>,
        parent_head_pc: u32,
        parent_exit_idx: usize,
    ) -> Self {
        TraceRecord {
            head_proto: proto,
            head_pc,
            entry_tags,
            ops: Vec::with_capacity(MAX_TRACE_LEN),
            closed: false,
            is_call_triggered: false,
            tfor_iter_ptr: None,
            tfor_val_tag: None,
            side_trace_parent: Some((parent_head_proto, parent_head_pc, parent_exit_idx)),
            self_link_kind: None,
            retfs: Vec::new(),
            downrec_close: None,
            field_ic_snapshot: None,
            result_tags: Vec::with_capacity(MAX_TRACE_LEN),
            field_slots: Vec::with_capacity(MAX_TRACE_LEN),
        }
    }

    /// Append an op. Returns `false` when the trace is full and
    /// recording should abort.
    pub fn push(&mut self, op: RecordedOp) -> bool {
        if self.ops.len() >= MAX_TRACE_LEN {
            return false;
        }
        self.ops.push(op);
        self.result_tags.push(RESULT_TAG_UNKNOWN);
        self.field_slots.push(FIELD_SLOT_UNKNOWN);
        true
    }
}

/// Outcome of a recording attempt — what `Vm::run` should do next.
#[derive(Debug)]
pub enum RecordOutcome {
    /// Recording is still in progress; keep dispatching as normal
    /// and continue recording the next op.
    InProgress,
    /// Recording closed cleanly; the trace is ready to compile.
    /// `Vm::run` should commit the record and continue interpreting.
    Closed,
    /// Recording exceeded `MAX_TRACE_LEN` or hit an un-recordable op.
    /// `Vm::run` should drop the record and resume interpretation.
    Aborted,
}
