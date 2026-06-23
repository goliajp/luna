//! Type definitions and small helpers extracted from `trace.rs` for
//! the luna-core / luna split boundary. See
//! `.dev/rfcs/v1.1-rfc-crate-split.md` §Migration Step 6.
//!
//! Everything here is cranelift-free by construction — the items
//! ultimately home to `luna-core` in Session C, while `trace.rs`
//! (the codegen pipeline) homes to `luna`. For now both files
//! live next to each other under `src/jit/` and re-export via
//! `mod.rs` + a `pub use super::trace_types::*;` in `trace.rs`
//! so `crate::jit::trace::*` paths remain compatible.

use crate::runtime::Gc;
use crate::runtime::function::Proto;
use crate::vm::isa::Inst;

/// Back-edge visit count after which a PC is promoted to a trace
/// head and recording begins. Tuned for benches in the 1k–10k
/// iteration range — too low and we record short traces that don't
/// pay back compile cost; too high and we never trace at all.
pub const TRACE_HOT_THRESHOLD: u32 = 64;

/// P12-S4 — call visit count after which a Proto is promoted to a
/// trace head at `pc=0` and recording begins. Separate from
/// [`TRACE_HOT_THRESHOLD`] so we can tune them independently — a
/// self-recursive function reaches its threshold via call counter
/// while its body's back-edges (if any) reach theirs via the
/// back-edge counter. Same value for now.
pub const CALL_HOT_THRESHOLD: u32 = 64;

/// Cap on the number of bytecode instructions captured in one trace.
/// Beyond this, recording aborts (the trace is too long to compile
/// usefully). PUC LuaJIT's default is 1024; luna starts conservative.
pub const MAX_TRACE_LEN: usize = 256;

/// Max inline depth for self-recursive `Op::Call` during recording.
/// Beyond this, the trace emits a real cranelift `call` to itself.
pub const MAX_INLINE_DEPTH: u8 = 16;

/// P16-A — recunroll threshold (mirrors LuaJIT `lj_jit.h:123` default
/// `recunroll=2`). The recorder counts how many ancestor frames share
/// the trace head's proto; when the count EXCEEDS this threshold AND
/// we're about to execute the head_pc on the head_proto, close the
/// trace with `TraceEnd::SelfLink`. Default 2 = inline 2 recursion
/// levels (so the recorded body covers 3-deep fib body per loop iter
/// after the lowerer's bump-base + branch-to-self tail).
pub const RECUNROLL_THRESHOLD: usize = 2;

/// P16-A — distinguishes the two self-link close shapes. UpRec
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
    /// outer trace; positive values come from S4 inlining.
    pub inline_depth: u8,
    /// P12-S9-A — recorder snapshot of the runtime variable count
    /// for ops whose B / C field is `0` (meaning "use stack top").
    /// - `Op::Call` with `C == 0`: snapshot of `top - A` AFTER the
    ///   call returns — i.e. the actual number of values the
    ///   callee returned this trip.
    /// - `Op::SetList` with `B == 0`: snapshot of `top - A` at the
    ///   op — i.e. the number of source slots `[A+1..top]`.
    /// - All other ops: `None`.
    /// S9-A only captures + tests; emit (S9-B/C) consumes this as
    /// a compile-time constant guarded by a runtime equality check.
    pub var_count: Option<u32>,
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
    /// P12-S4-step2 — `true` if the recording was fired by a
    /// trace-on-call trigger (`begin_call`'s Lua callee arm), as
    /// opposed to a back-edge trigger (`Op::Jmp` neg / `Op::ForLoop`).
    /// Affects the dispatcher's close detection: call-triggered
    /// traces close on **any** re-entry of `(head_proto, head_pc)`
    /// (single-pass through the function body), while loop-triggered
    /// traces require `cur_depth == 0` so a nested call to the
    /// containing loop's function doesn't prematurely close.
    pub is_call_triggered: bool,
    /// P12-S12-B-v5 — generic-for iter fn pointer snapshot.
    /// Populated by `Op::TForLoop`'s recorder trigger when
    /// `R[A]` is `Value::Native`. Lets the lowerer specialise
    /// `Op::TForCall` emit on `ipairs_iter` (inline Table aget
    /// via `TABLE_ARRAY_PTR_OFFSET` / `TABLE_ASIZE_OFFSET` —
    /// skip the `luna_jit_op_tforcall` C call entirely). `None`
    /// for non-generic-for traces or when the recorder fires
    /// for a non-Native iter.
    pub tfor_iter_ptr: Option<usize>,
    /// P12-S12-C v3 — snapshot of `R[A+5]` (the iter's value
    /// slot) tag at recorder fire. v5 ipairs inline aget emits a
    /// runtime guard `val_tag == expected_tag` (or Nil for the
    /// loop-end branch); a mismatch deopts to interp. Without the
    /// guard, mixed-tag arrays (e.g. `{'a', 1, 'c'}`) would let
    /// v2's Str-specialised spill pack non-Str raw bits as a Str
    /// pointer → garbage. `None` for non-generic-for traces or
    /// when the snapshot slot isn't reachable.
    pub tfor_val_tag: Option<u8>,
    /// P15-A v1 — if set, this trace is a SIDE TRACE: it was
    /// triggered by a parent trace's hot side-exit, NOT by the
    /// usual back-edge / call-trigger paths. The tuple is
    /// `(parent_head_proto, parent_head_pc, parent_exit_idx)`,
    /// uniquely identifying the parent's `CompiledTrace` and the
    /// `exit_hit_counts` slot that crossed [`HOTEXIT_THRESHOLD`].
    /// `None` for primary traces. v1 only records the metadata; v2
    /// reads it to wire the parent's exit-branch indirection
    /// pointer to the side trace's entry once it compiles.
    pub side_trace_parent: Option<(Gc<Proto>, u32, usize)>,
    /// P16-A — set by the recorder cycle catch when a same-proto
    /// ancestor count exceeds [`RECUNROLL_THRESHOLD`] at head_pc on
    /// head_proto. Drives the lowerer's `TraceEnd::SelfLink` close
    /// shape (snapshot-restore + bump-base + branch-to-self), and
    /// inhibits `is_inline_abort_close` even though the recorded
    /// body has depth>0 ops. `None` for all non-self-link closes
    /// (Call truncation, ForLoop, Return, InlineAbort).
    pub self_link_kind: Option<SelfRecKind>,
}

impl TraceRecord {
    /// Start a fresh recording at `head_pc` of `proto`. The
    /// `entry_tags` snapshot pins the per-slot `Value` tag at the
    /// moment recording fires; pass an empty vec for test
    /// harnesses that don't have a live stack to snapshot.
    /// `is_call_triggered = true` only when fired by a trace-on-call
    /// (S4-step0); back-edge triggers pass `false`.
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
        }
    }

    /// P15-A v1 — start a SIDE trace recording at a hot side-exit's
    /// `cont_pc`. The trace's head_proto is the proto interp resumed
    /// in after the side-exit fired (today: same as the parent's
    /// head_proto, since trace JIT only inlines self-recursive
    /// calls — see `docs/rfcs/20260621-side-trace-tree/design.md`).
    /// `parent_*` identifies the parent `CompiledTrace`'s
    /// `exit_hit_counts` slot so v2 can wire the back-pointer.
    ///
    /// `is_call_triggered = false` for side traces — the close
    /// detection runs like a back-edge trigger (cur_depth==0 +
    /// pc==head_pc), and per S13-H the discard heuristic for short
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
        }
    }

    /// Append an op. Returns `false` when the trace is full and
    /// recording should abort.
    pub fn push(&mut self, op: RecordedOp) -> bool {
        if self.ops.len() >= MAX_TRACE_LEN {
            return false;
        }
        self.ops.push(op);
        true
    }
}

/// Outcome of a recording attempt — what `Vm::run` should do next.
#[derive(Debug)]
pub enum RecordOutcome {
    /// Recording is still in progress; keep dispatching as normal
    /// and continue recording the next op.
    InProgress,
    /// Recording closed cleanly; the trace is ready to compile in S2.
    /// `Vm::run` should commit the record and continue interpreting.
    Closed,
    /// Recording exceeded `MAX_TRACE_LEN` or hit an un-recordable op.
    /// `Vm::run` should drop the record and resume interpretation.
    Aborted,
}

/// Native entry point for a compiled trace.
///
/// **S2.B step 2 ABI** (this commit):
///
/// ```text
/// fn(reg_state: *mut i64) -> i64
/// ```
///
/// - `reg_state` points to a caller-managed buffer of
///   `head_proto.max_stack` `i64` slots. The trace reads its live
///   inputs from this buffer at entry and writes back any modified
///   regs before returning. Each slot holds the raw 8-byte payload
///   of a Lua `Value` — type tags are out of scope until step 3 adds
///   side-exit guards (the dispatcher is expected to bias values so
///   that the trace's recorded type assumptions hold).
/// - Return value = continuation PC. A clean loop close (control
///   returns to the trace's `head_pc`) returns `head_pc as i64`.
///   Step 3 will add side-exit returns for failing guards
///   (`failing_pc as i64`).
///
/// Step 1's `() -> i64` sig has been retired — only the lowerer
/// itself has been refined; the rest of the recording pipeline
/// (`Vm.active_trace`) is untouched.
// SAFETY: `TraceFn` is the ABI of native code emitted by the Cranelift lowerer (see `jit_backend::trace`); callers guarantee the `*mut i64` points to a reg_state buffer of size `window_size` and survive the trace call.
pub type TraceFn = unsafe extern "C" fn(*mut i64) -> i64;

/// What tag a register holds at the trace's exit point (relative
/// to its entry tag). Stored per register in `CompiledTrace.exit_tags`
/// so the dispatcher knows how to re-pack the i64 payload back into
/// a `Value` after the trace runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitTag {
    /// Slot is untouched by the trace — restore the entry tag.
    Untouched,
    /// Trace writes an `Int` value to this slot (arith result,
    /// LoadI, Len, ForLoop step / count / visible-var).
    Int,
    /// Trace writes a `Float` bit-pattern to this slot (LoadF
    /// result, Float arith on two Float operands).
    Float,
    /// Trace writes a `Table` ptr to this slot (NewTable result).
    Table,
    /// P12-S4-step2c — trace writes a `Closure` ptr to this slot.
    /// Today the only producer is `Op::GetUpval` whose result is
    /// inferred (via `infer_upval_exit`) to feed an `Op::Call` as
    /// the call target — the upval *must* be a closure for that
    /// dispatch to be sound.
    Closure,
    /// P12-S6-A1 — trace actively writes Nil to this slot (the only
    /// producer today is `Op::LoadNil`; raw payload is 0). The
    /// dispatcher restores `Value::Nil` regardless of the slot's
    /// entry tag. Split out from `Untouched` so a LoadNil writer
    /// over an Int/Float/Table entry slot doesn't get mis-packed
    /// back as the entry type.
    Nil,
    /// P12-S12-C v2 — trace writes a `Str` ptr to this slot (LoadK
    /// of a Str constant, Move from a Str slot, or Concat result).
    /// Dispatcher repacks as `Value::Str(Gc::from_ptr(raw))`.
    Str,
}

/// Derive an [`ExitTag`] vector from a per-slot `RegKind` snapshot.
/// `Unset` slots restore via the dispatcher's entry tags (trace
/// didn't touch them); writers (including `Nil`, see P12-S6-A1)
/// translate one-to-one to a tag the dispatcher packs without
/// consulting the entry tag.
/// P13-S13-E — fast-path classification of an `exit_tags`
/// vector. Lets the dispatcher's restore loop skip per-slot
/// match-arm dispatch when the entire vector resolves to a
/// single trivial pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagResKind {
    /// Every slot's tag is `Untouched`. The trace didn't override
    /// any slot's exit type; vm.stack already holds the right
    /// values from either marshal-in or trace spill helpers.
    /// Dispatcher skips the restore loop.
    AllUntouched,
    /// Every slot's tag is `Int`. Dispatcher writes
    /// `Value::Int(reg_state[i])` per slot without a match arm.
    AllInt,
    /// Anything else — fall back to the original loop with
    /// per-iter match.
    Mixed,
}

/// Walk an `exit_tags` slice and classify it for the
/// dispatcher fast path.
#[doc(hidden)]
pub fn classify_exit_tags(tags: &[ExitTag]) -> TagResKind {
    if tags.iter().all(|t| matches!(t, ExitTag::Untouched)) {
        return TagResKind::AllUntouched;
    }
    if tags.iter().all(|t| matches!(t, ExitTag::Int)) {
        return TagResKind::AllInt;
    }
    TagResKind::Mixed
}

/// A trace compiled by S2's lowerer and ready to be dispatched into
/// at its head PC. Owned by `Proto.traces`; the underlying mmap is
/// kept alive by the `Vm.jit_handles` Vec for the Vm's lifetime,
/// just like the method JIT's compiled functions.
pub struct CompiledTrace {
    /// Pc the trace dispatches at (matches the recorder's `head_pc`).
    pub head_pc: u32,
    /// Native entry function (mmap'd machine code, valid for the Vm's
    /// lifetime as long as the backing handle is kept).
    pub entry: TraceFn,
    /// Number of ops in the source `TraceRecord`. Diagnostic only;
    /// tuning will gate re-record vs. recompile based on this.
    pub n_ops: u32,
    /// `true` iff the dispatcher can safely invoke this trace.
    /// False when the trace has ops the lowerer can't predict the
    /// exit type for (today: `Op::GetI` — the helper returns a
    /// raw payload that may be Int or Table or Float; without
    /// runtime tag info the dispatcher can't repack the slot).
    /// Non-dispatchable traces still compile and stay cached so
    /// a future dispatcher with richer marshalling can pick them
    /// up — they just don't run today.
    pub dispatchable: bool,
    /// P12-S4-step3a — size of the reg_state buffer the dispatcher
    /// must allocate when calling `entry`. Today always equals
    /// `head_proto.max_stack` (the trace covers only the head
    /// frame). S4-step3b's inline emit pushes this past `max_stack`
    /// to fit additional inlined frames whose register windows sit
    /// at `offsets[i]..offsets[i] + max_stack` within the buffer.
    /// The dispatcher's marshal-in still only writes [0..max_stack)
    /// — depth>0 slots start initialized to zero, and the trace's
    /// own GetUpval / arith fills them as it runs.
    pub window_size: u32,
    /// Per-register exit tag of length `window_size`. Indexed by
    /// position within the trace's reg_state_buf. The dispatcher
    /// consults this to pack `reg_state[i]` back into a `Value`
    /// after the trace returns at the **clean tail** (head_pc or
    /// call-truncation pc). See [`ExitTag`] for the semantics.
    /// `Rc<[]>` so the dispatcher's per-dispatch lookup is a cheap
    /// refcount bump, not a Vec heap clone (fib_28 dispatches 1M×
    /// — clone cost dominates without this).
    pub exit_tags: std::rc::Rc<[ExitTag]>,
    /// P13-S13-E — classification of the global `exit_tags` for
    /// the dispatcher's restore-loop fast path. The dispatcher
    /// dispatches on this when `site_id == 0` AND
    /// `per_exit_tags.find(cont_pc)` misses (the common
    /// back-edge / clean-tail exit shape):
    /// - `AllUntouched` → skip the restore loop entirely (trace
    ///   touched no slots; vm.stack already holds the right
    ///   values from entry, possibly modified by spill helpers)
    /// - `AllInt`       → `vm.stack[base+i] = Value::Int(reg_state[i])`
    ///   per slot, no per-iter match
    /// - `Mixed`        → original match-arm loop
    pub global_tag_res_kind: TagResKind,
    /// P12-S12-C v3 — compile-time snapshot of `entry_tags` from the
    /// `TraceRecord`. The trace's IR + `current_kinds` propagation
    /// are specialised to these tags; if the runtime entry tags
    /// differ, the dispatcher must skip dispatch (fall back to
    /// interp) — otherwise the trace would treat e.g. a Str ptr
    /// slot as Int and produce garbage. `Rc<[]>` to match the
    /// other tag arrays' cheap-clone idiom.
    pub entry_tags: std::rc::Rc<[u8]>,
    /// P12-S4-step2c — per side-exit `exit_tags`. Each entry is
    /// `(continuation_pc, exit_tags)`; when the trace returns a PC
    /// matching an entry, the dispatcher uses that vector instead of
    /// the clean-tail `exit_tags`. This makes side-exits that fire
    /// **before** later writers (`GetUpval` is the today motivator)
    /// restore the affected slot as `Untouched` (carry entry tag)
    /// rather than pack with a tag the slot hasn't actually become.
    /// Empty when no side-exit needs a different vector than the
    /// clean tail (e.g. plain numeric loops with no GetUpval).
    pub per_exit_tags: std::rc::Rc<[(u32, std::rc::Rc<[ExitTag]>)]>,
    /// P12-S4-step4b-C-2 — per inline side-exit metadata, indexed by
    /// `site_idx`. Each entry carries the side-exit's `cont_pc`,
    /// the per-slot `exit_tags` snapshot (sized to `window_size` so
    /// every materialised frame's window is restored), and the
    /// frame-materialise `chain` to push.
    ///
    /// fib has SIBLING self-recursive Calls (pc7, pc11) and EVERY
    /// depth's cmp lands at the same `cont_pc` — keying the lookup
    /// by `cont_pc` alone (the v2 attempt) collapsed all those
    /// distinct chains onto one entry. The trace IR encodes the
    /// firing site's `(site_idx + 1)` in the upper 32 bits of the
    /// returned i64 so the dispatcher disambiguates O(1).
    ///
    /// Empty when no cmp@d>0 fires in the trace. The IR pre-bakes
    /// each `chain`'s raw pointer (`Rc::as_ptr`) at compile time;
    /// the `Rc` clones in this field keep the slice alive for the
    /// trace's mmap lifetime (Proto.traces owns the CompiledTrace).
    pub per_exit_inline: std::rc::Rc<[InlineSideExit]>,
    /// P15-prep — per-exit hit counter (LuaJIT-study foundation for
    /// future side trace work). Length and layout:
    /// - `[0..per_exit_inline.len())`: parallel to per_exit_inline
    ///   (indexed by `site_id - 1` in the dispatcher).
    /// - `[per_exit_inline.len()..per_exit_inline.len()+per_exit_tags.len())`:
    ///   parallel to per_exit_tags (indexed by find-by-cont_pc order).
    /// - Last slot: global / clean-tail exit (when site_id == 0 AND
    ///   per_exit_tags.find misses).
    ///
    /// `Rc<[Cell<u32>]>` so the dispatcher can increment without a
    /// mutable borrow on the CompiledTrace. Vm's
    /// `trace_exit_hit_distribution()` aggregates this for probe use.
    pub exit_hit_counts: std::rc::Rc<[std::cell::Cell<u32>]>,
    /// P15-A v2-A — per-exit raw side-trace function pointer. Same
    /// length / layout as [`Self::exit_hit_counts`]. `null` means
    /// "no side trace compiled for this exit yet"; non-null means a
    /// child side trace's entry fn lives at this pointer and v2-B/C
    /// will wire the parent's IR at each exit site to read this Cell
    /// and indirect-call when non-null.
    ///
    /// `Cell<*const u8>` (not Atomic) since the Vm is single-
    /// threaded — see RFC Q2. The pointer's stability is owned by
    /// the child side trace's `TraceHandle` in `TRACE_JIT_HANDLES`
    /// (thread-local Vec), which persists for the thread lifetime.
    ///
    /// **Send/Sync invariant**: `Cell<*const u8>` is not Sync, but
    /// `CompiledTrace` was never required to be Sync (it lives in
    /// `Proto.traces: RefCell<Vec<CompiledTrace>>` on the runtime
    /// path). Adding this field doesn't tighten that.
    pub exit_side_trace_ptrs: std::rc::Rc<[std::cell::Cell<*const u8>]>,
    /// P15-A v2-C-A2 — per-`per_exit_tags`-entry side-trace cell.
    /// Same length as `per_exit_tags`; the IR at the corresponding
    /// `emit_store_back_and_return_pc` callsite (immediately after
    /// `per_exit_kinds.push`) bakes this cell's heap address. Same
    /// semantics as [`InlineSideExit::side_trace_ptr`] but with
    /// `kind = SIDE_SENT_KIND_TAG` and `local = tag_idx`.
    pub tags_side_trace_ptrs: std::rc::Rc<[Box<std::cell::Cell<*const u8>>]>,
    /// P15-A v2-C-A2 — singleton cell shared by every GLOBAL-kind
    /// callsite (clean-tail return, Call truncation, ForLoop /
    /// TForLoop exits, generic err deopts, etc.). All such sites'
    /// IR bakes the same heap address; the close handler writes
    /// the child entry ptr here for `parent_exit_idx ==
    /// per_exit_inline.len() + per_exit_tags.len()` (the
    /// `exit_hit_counts` layout's last slot).
    pub global_side_trace_ptr: Box<std::cell::Cell<*const u8>>,
    /// P15-A v2-C-A1 — when a child side trace compiles for any
    /// of this trace's hot exits, the close handler inserts
    /// `(child.head_pc, child_traces_idx)` here. v2-C-A3's
    /// dispatcher uses this for an O(1) lookup of the side trace's
    /// own [`CompiledTrace`] when the sentinel bit on `raw_ret`
    /// (introduced by v2-C-A2) flags a side-trace return — so
    /// [`decode_exit_shape`] can be called with the SIDE TRACE's
    /// `per_exit_inline` / `per_exit_tags` / `exit_tags` instead
    /// of the parent's.
    ///
    /// Value is an **index** into `head_proto.traces` (the same
    /// proto this `CompiledTrace` lives in — trace JIT only fires
    /// side traces from self-recursive parents today, so child +
    /// parent share `head_proto`). Storing an index instead of a
    /// raw pointer dodges the `Vec<CompiledTrace>` realloc-
    /// invalidation pitfall: `proto.traces.push` doesn't reorder,
    /// only appends, so an index assigned at compile time stays
    /// valid for the trace's lifetime.
    ///
    /// `RefCell<HashMap<u32, u32>>` because the close handler
    /// holds only `&CompiledTrace` (the parent's traces borrow is
    /// immutable while we're walking it to find the parent_ct).
    pub side_trace_cache: std::cell::RefCell<std::collections::HashMap<u32, u32>>,
    /// P15-A v2-D-A8 — fast-path short-circuit hint for the
    /// dispatcher's tentative-decode + cell-load + check path. Set
    /// to `true` by the close handler when ANY of this trace's
    /// `exit_side_trace_ptrs` cells gets wired (i.e., the first
    /// time a child side trace compiles + the A5-C shape gate
    /// passes). Stays `true` for the trace's lifetime — once any
    /// side trace exists, the dispatcher must perform the per-
    /// exit check on every dispatch.
    ///
    /// When `false`, the dispatcher skips the tentative decode +
    /// cell load + child lookup entirely, falling straight through
    /// to the cheap parent decode + writeback. Trims fib_10_x10k-
    /// class tight-trace workloads' per-dispatch overhead from the
    /// double-decode pattern to a single `Cell::get()`.
    ///
    /// `Cell<bool>` so the close handler can flip the flag through
    /// only an `&CompiledTrace` borrow (the parent's `traces`
    /// borrow is immutable while the close handler walks).
    pub has_any_side_wired: std::cell::Cell<bool>,
    /// P13-S13-G v2 — `true` iff this trace closes at a
    /// `TraceEnd::InlineAbort` (depth>0 op the lowerer can't
    /// continue past: depth past `MAX_INLINE_DEPTH`, non-self
    /// Call@d>0, ForLoop@d>0, TForLoop@d>0, or proto mismatch).
    /// Such traces compile but pin `dispatchable=false` —
    /// dispatching them would resume interp at a depth>0 PC
    /// without the matching CallFrames the trace inlined past
    /// (S4-step4b's frame mat helper can synthesise these but
    /// isn't wired up for InlineAbort exits yet — that's the
    /// S13-G v2 follow-up). Vm's `trace_inline_abort_count`
    /// tallies these so future-tuning sees what bench cells
    /// would benefit from the frame-mat unlock.
    pub is_inline_abort_close: bool,
    /// P13-S13-G v2.5 — if `dispatchable == false`, the static
    /// label of the emit-pass site that flipped it. Lets a probe
    /// distinguish among the six places trace.rs pins dispatch
    /// off (GetI / GetTable / GetUpval inference fail, TForCall
    /// slow-path, length gate, InlineAbort gate). `None` if the
    /// trace IS dispatchable, the first label otherwise.
    pub dispatch_off_reason: Option<&'static str>,
    /// P12-S5-A — number of NewTable sites in this trace whose
    /// final `EscapeState` is `EscapeState::Sinkable` after
    /// S5-B's pre-emit demotion pass. Vm's
    /// `trace_sinkable_seen_count` tallies these for telemetry.
    pub sinkable_sites_seen: u32,
    /// P14-S14-B v1 — number of `AccumSite`s with `BufferState::Bufferable`
    /// detected by `detect_accumulators`. v1 only counts; v2+ will use
    /// the sites for buffered emit. Vm's `trace_accum_bufferable_seen_count`
    /// tallies these for probe visibility.
    pub accum_bufferable_seen: u32,
    /// P12-S5-B — number of Sinkable sites this trace's emit
    /// actually allocated virt slot Variables for (i.e., took the
    /// no-heap-alloc path). Always `<= sinkable_sites_seen`. Bumps
    /// `Vm::trace_sunk_alloc_count` on compile success.
    pub sunk_alloc_seen: u32,
    /// P12-S5-C — number of (site × cmp side-exit) pairs in this
    /// trace's IR that emit the materialise helper. Each pair is
    /// "this cmp's side-exit reconstructs site X's heap Table".
    /// Static count; the runtime number of helper calls depends
    /// on dispatch shape (which side-exits actually fire).
    pub materialize_emit_count: u32,
    /// P12-S7-A — number of `Op::Closure` ops this trace's emit
    /// lowered to a `luna_jit_op_closure` helper call. Each
    /// closure-creating op replaces a `Heap::new_closure_inline`
    /// allocation, which dwarfs the dispatcher's marshal overhead;
    /// the length-gate skip below treats `closure_seen > 0` the
    /// same as `sunk_alloc_seen > 0` (don't gate short traces).
    pub closure_seen: u32,
    /// P15-A v2-E — sorted unique list of slot indices that ANY
    /// op in this trace's body WRITES (post `inline_depth` offset).
    /// Computed at compile via `compute_body_writes`; consumed
    /// by the v2-E smart side-trace gate at child compile to
    /// detect read-before-write live-in registers that would
    /// re-read the parent's stale exit value across the child's
    /// internal-loop iters (see s12_step_b bug analysis).
    pub body_writes: Box<[u32]>,
}

/// P12-S4-step4b-C-2 — per inline cmp@d>0 side-exit record. See
/// [`CompiledTrace::per_exit_inline`] for the shape rationale.
#[derive(Clone, Debug)]
pub struct InlineSideExit {
    /// PC the interpreter resumes at after the side-exit fires.
    /// Mirrors the innermost frame's `pc` in `chain`.
    pub cont_pc: u32,
    /// PC to write on the trace head frame when the side-exit
    /// fires — the depth-0 frame's resume point after ITS own Call
    /// that entered depth 1. Without this update, the trace head
    /// frame's pc stays at `head_pc` (where the dispatcher entered);
    /// once the inlined chain pops, interp resumes the trace head
    /// at pc=0 and immediately self-Calls again → infinite dispatch
    /// loop. Captured at emit time as the outermost `Op::Call`'s
    /// `pc + 1` from the live `call_chain`.
    pub head_resume_pc: u32,
    /// Slot-by-slot `ExitTag` snapshot at the side-exit moment.
    /// Length = `window_size` — covers caller + every inlined
    /// frame's register window.
    pub exit_tags: std::rc::Rc<[ExitTag]>,
    /// Frames to push onto `vm.frames` (outermost = depth 1 first,
    /// innermost = depth `len()` last). The innermost frame's `pc`
    /// is overwritten to the side-exit PC at compile time so the
    /// helper stays PC-agnostic.
    pub chain: std::rc::Rc<[FrameMaterializeInfo]>,
    /// P15-A v2-C-A2 — raw `*const u8` (entry fn pointer of a child
    /// side trace) for THIS inline cmp@d>0 side-exit. The IR at the
    /// `emit_store_back_and_return_site` call site loads this cell
    /// BEFORE the encoded-return path: non-null → store-back +
    /// `call_indirect` into the child + OR sentinel(INLINE, site_idx)
    /// into bits 56..=63 of the child's return + return; null → run
    /// the existing encoded-return path.
    ///
    /// `Box<Cell<*const u8>>` (not embedded Cell) so the cell's HEAP
    /// address is stable for the IR's `iconst`-baked load. Moving
    /// the Box (e.g. into `Rc<[]>` via `.collect`) doesn't move the
    /// cell. Single-threaded Vm so `Cell` is sound.
    pub side_trace_ptr: Box<std::cell::Cell<*const u8>>,
}

/// P15-A v0 — hot side-exit detection threshold. Exits whose hit
/// count crosses this value are reported by `Vm::hot_exit_iter` as
/// side-trace candidates. LuaJIT 2.1's default is 10, but short
/// workloads (binary_trees_d4_x200 = 200 outer iters, each calling
/// make/itemcheck a small handful of times) don't reach 10 hot
/// hits before the run ends. v2-G drops to 3 so short workloads
/// also get a chance to wire side traces.
pub const HOTEXIT_THRESHOLD: u32 = 2;

/// P15-A v2-C-A2 — sentinel kind tags for side-trace returns.
/// When a parent trace's IR detects a wired child side-trace cell
/// non-null at a side-exit and tail-calls into the child, it OR's
/// a 7-bit sentinel into the upper bits of the child's return value
/// (bit 63 = side-trace marker, bits 56..=62 = `encode_side_sentinel
/// (kind, local)`). The dispatcher reads the marker to know it must
/// re-decode the body using the SIDE TRACE's shape inputs, not the
/// parent's. The kind is informational (debug + close-handler routes
/// the right cell write); `local` disambiguates among multiple wired
/// cells of the same kind (e.g. several inline cmp@d>0 sites).
pub const SIDE_SENT_KIND_INLINE: u8 = 1;
/// Sentinel kind for tag-cell side-traces (typed-register exits).
pub const SIDE_SENT_KIND_TAG: u8 = 2;
/// Sentinel kind for global-cell side-traces (env-table exits).
pub const SIDE_SENT_KIND_GLOBAL: u8 = 3;

/// P15-A v2-C-A2 — encode a `(kind, local)` pair into a 7-bit
/// sentinel code that fits in `raw_ret`'s bits 56..=62. Layout:
/// upper 2 bits = kind (1..=3), lower 5 bits = local index. A local
/// index `>= 32` is truncated; the close handler caps tag-cell
/// allocation at 32 to avoid sentinel collisions. The dispatcher
/// uses the full 7-bit value as the key into the parent's
/// `side_trace_cache`.
pub fn encode_side_sentinel(kind: u8, local: u32) -> u32 {
    debug_assert!(
        kind >= 1 && kind <= 3,
        "kind must be SIDE_SENT_KIND_* (1..=3)"
    );
    ((kind as u32 & 0x3) << 5) | (local & 0x1F)
}

/// P15-A v2-C-A6 — env-gated probe switch. `LUNA_V2C_PROBE=1` (any
/// non-empty value) turns on the side-trace dispatch probes (IR
/// side-entry, dispatcher A3 decode, frame.pc set). Off by default
/// so production runs pay no overhead — even the IR-emitted probe
/// call is conditional on the probe helper itself short-circuiting
/// when the OnceLock resolves to `false`.
static V2C_PROBE_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
/// True iff the side-trace dispatch probes are enabled via
/// `LUNA_V2C_PROBE=1`. Diagnostic-only; production builds keep this
/// off so the IR-emitted probe call short-circuits cheaply.
pub fn v2c_probe_enabled() -> bool {
    *V2C_PROBE_ON.get_or_init(|| {
        std::env::var("LUNA_V2C_PROBE")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
    })
}

/// P15-A v0 — one hot side-exit candidate surfaced by
/// `Vm::hot_exit_iter`. The walker fills this from one
/// [`CompiledTrace`]'s `exit_hit_counts` slot whose value passed
/// [`HOTEXIT_THRESHOLD`].
///
/// `head_proto` + `head_pc` identify the *parent* trace; `exit_idx`
/// indexes into the parent's `exit_hit_counts` (same layout the
/// dispatcher uses to bump). `cont_pc` is where the interpreter
/// resumed after the side-exit; this is the side trace's natural
/// entry PC. `exit_tags` is the compile-time slot-shape snapshot the
/// side trace would inherit as its entry tags.
#[derive(Clone, Debug)]
pub struct HotExitInfo {
    /// The trace head's Proto. `head_proto.traces` owns the parent
    /// [`CompiledTrace`]; combined with `head_pc` it uniquely
    /// identifies which trace this exit belongs to.
    pub head_proto: Gc<Proto>,
    /// PC of the parent trace's head (== the entry the dispatcher
    /// looks up under `cl.proto.traces`).
    pub head_pc: u32,
    /// Index into the parent's `exit_hit_counts`. Layout:
    /// - `[0..per_exit_inline.len())`: inline cmp@d>0 side-exits
    /// - `[per_exit_inline.len()..per_exit_inline.len() + per_exit_tags.len())`:
    ///   per-cont_pc side-exits (GetUpval-style)
    /// - last slot: global clean-tail / back-edge fallback
    pub exit_idx: usize,
    /// Saturating count from `exit_hit_counts[exit_idx]` at the
    /// moment of the walk. Always `>= HOTEXIT_THRESHOLD`.
    pub hits: u32,
    /// PC the interpreter resumed at after this side-exit fired.
    /// Inline side-exits read from `InlineSideExit.cont_pc`;
    /// per_exit_tags entries from their `(cont_pc, _)` pair; the
    /// global slot reports `head_pc` (the clean-tail back-edge
    /// returns to the trace's head, where dispatch can re-enter).
    pub cont_pc: u32,
    /// Slot-shape snapshot at the exit moment, reused as the side
    /// trace's entry_tags. Inline side-exits cover the full
    /// `window_size` (caller + inlined frames); per_exit_tags
    /// entries cover only the caller's `max_stack`; the global
    /// slot exposes the clean-tail `exit_tags` (caller window only).
    pub exit_tags: std::rc::Rc<[ExitTag]>,
}

/// P12-S4-step4b — one Lua frame to push when a depth>0 side-exit
/// fires. Constructed at trace compile time from the recorded
/// `Op::Call` chain's `A` field (caller's `R[A]` = function slot) and
/// the inlined callee's `c` field (`nresults`). `pc` is the address
/// the helper writes onto the freshly-pushed frame so the interp
/// resumes at the right offset inside the callee body.
///
/// `repr(C)` because the trace's IR loads the array via raw pointer
/// arithmetic; Rust's default `repr` doesn't guarantee field order.
/// All-`Copy` fields with no padding inside each field — 12 bytes
/// per entry on amd64.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FrameMaterializeInfo {
    /// Stack offset (relative to the trace head's `frame.base`) of
    /// the callee's first register slot. The new frame's `base` is
    /// `head_frame.base + base_offset`; its `func_slot` is one below.
    pub base_offset: u32,
    /// PC to write on the freshly-pushed frame. For inner frames
    /// (not the innermost) this is the caller's Call.pc + 1 so the
    /// interp resumes after the Call instruction. For the innermost
    /// frame (the one the side-exit fires inside) the dispatcher
    /// overrides this with the actual side-exit PC — keeps the
    /// helper PC-agnostic per the RFC's "helper doesn't know which
    /// frame is innermost" rule.
    pub pc: u32,
    /// PUC `nresults`: how many return values the caller expects
    /// from this call (encoded as `Op::Call`'s C - 1). step4b-C's
    /// pre-emit pass bails if any inlined Call has nresults != 1
    /// (Op::Return1 copy-back assumes one value).
    pub nresults: i32,
}

impl std::fmt::Debug for CompiledTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledTrace")
            .field("head_pc", &self.head_pc)
            .field("n_ops", &self.n_ops)
            .field("dispatchable", &self.dispatchable)
            .field("exit_tags", &self.exit_tags)
            .field("entry", &"<fn>")
            .finish()
    }
}

/// Result of attempting to lower a closed [`TraceRecord`] to native
/// code. Most failure cases are recoverable — the recorder bumps the
/// head PC's failure count and refuses to re-record until the
/// threshold rolls over again.
#[derive(Debug)]
pub enum CompileOutcome {
    /// Trace compiled; the cached entry is ready for dispatch in S3.
    Compiled,
    /// Some op in the trace falls outside S2's whitelist (e.g. a
    /// metamethod-bearing operand, or a yet-unsupported opcode).
    /// The record is dropped; the head PC remembers the rejection.
    UnsupportedOp,
    /// Cranelift signaled an error during code emission. Should be
    /// rare in practice — usually a programmer error in the lowerer.
    BackendError,
}

/// P15-A v2-C-A5-C — return `true` iff `child_entry_tags` is
/// compatible with `parent_exit_tags` (the parent's per-exit tag
/// snapshot at the slot the side trace was wired to). Used by
/// the close handler to gate the side-trace ptr write: only write
/// when shapes match so the future `call_indirect` (v2-C-A2 redo)
/// is guaranteed to feed the child reg_state values whose tags
/// agree with the child's `compile_entry_tags`.
///
/// `Untouched` slots in `parent_exit_tags` mean the parent didn't
/// override that slot during execution — its tag at parent's
/// exit equals its tag at parent's entry. The child's recorder
/// snapshotted from the same vm.stack at parent's exit, so for
/// those slots `child_entry_tags[i] == parent_compile_entry_tags
/// [i]` should hold.
pub fn exit_tags_match_entry_tags(
    child_entry_tags: &[u8],
    parent_exit_tags: &[ExitTag],
    parent_compile_entry_tags: &[u8],
) -> bool {
    let n = parent_exit_tags.len();
    if child_entry_tags.len() < n {
        return false;
    }
    for i in 0..n {
        let expected = match parent_exit_tags[i] {
            ExitTag::Untouched => {
                if i < parent_compile_entry_tags.len() {
                    parent_compile_entry_tags[i]
                } else {
                    // Parent didn't capture an entry tag here
                    // (inlined-frame scratch slot). Child can't
                    // safely consume — bail.
                    return false;
                }
            }
            ExitTag::Int => crate::runtime::value::raw::INT,
            ExitTag::Float => crate::runtime::value::raw::FLOAT,
            ExitTag::Table => crate::runtime::value::raw::TABLE,
            ExitTag::Closure => crate::runtime::value::raw::CLOSURE,
            ExitTag::Nil => crate::runtime::value::raw::NIL,
            ExitTag::Str => crate::runtime::value::raw::STR,
        };
        if child_entry_tags[i] != expected {
            return false;
        }
    }
    true
}

/// P15-A v2-C-A0 — decoded exit shape. Returned by
/// [`decode_exit_shape`]. Carries the per-exit metadata the
/// dispatcher's restore loop needs: the resume PC, the
/// `exit_hit_counts` slot index for the side-trace trigger
/// counter, the per-slot exit-tag array to interpret reg_state
/// through, and a flag for the global classified-restore fast
/// path.
///
/// The lifetime ties `exit_tags_for_pc` to whichever input slice
/// the decode picked from (one of the `CompiledTrace` fields).
/// The dispatcher's per_exit_inline / per_exit_tags / exit_tags
/// Rc clones from the per-dispatch lookup keep them alive for
/// the dispatch.
pub struct DecodedExit<'a> {
    /// Pc the interpreter should resume at after the trace exit.
    pub cont_pc: u32,
    /// Stable id of the exit site (used to key per-site counters / caches).
    pub site_id: u32,
    /// Index into `exit_hit_counts` for the side-trace trigger counter.
    pub exit_hit_idx: usize,
    /// Per-slot exit-tag array describing how to interpret saved register
    /// state for this exit.
    pub exit_tags_for_pc: &'a [ExitTag],
    /// True when the global classified-restore fast path applies.
    pub using_global_exit_tags: bool,
}

/// P15-A v2-C-A0 — decode a trace's i64 return value into the
/// per-exit shape the dispatcher needs to restore vm.stack +
/// bump the hit counter.
///
/// Pure function over the input slices — the dispatcher passes
/// the parent's `per_exit_inline` / `per_exit_tags` / `exit_tags`;
/// v2-C-A3 will call it again with the side trace's same fields
/// when bit 63 of `raw_ret` is set (the sentinel introduced by
/// v2-C-A2). Factored out of the inlined dispatcher block in
/// `Vm::run` for that future reuse — no behavior change vs the
/// inlined form.
///
/// Layout reminder (from `CompiledTrace::exit_hit_counts`):
/// - `[0..inline.len())` — inline cmp@d>0 sites, indexed by
///   `site_id - 1` (1-based encoding lets `site_id == 0` mean
///   "non-inline").
/// - `[inline.len()..inline.len() + tags.len())` — per_exit_tags
///   in find-by-cont_pc order.
/// - Last slot — global / clean-tail fallback.
pub fn decode_exit_shape<'a>(
    raw_ret: u64,
    per_exit_inline: &'a [InlineSideExit],
    per_exit_tags: &'a [(u32, std::rc::Rc<[ExitTag]>)],
    exit_tags: &'a [ExitTag],
) -> DecodedExit<'a> {
    let site_id = (raw_ret >> 32) as u32;
    let cont_pc = (raw_ret & 0xFFFF_FFFF) as u32;
    let inline_n = per_exit_inline.len();
    if site_id > 0 {
        let idx = (site_id - 1) as usize;
        debug_assert!(
            idx < inline_n,
            "site_idx out of range (idx={} inline_n={})",
            idx,
            inline_n
        );
        debug_assert_eq!(
            per_exit_inline[idx].cont_pc, cont_pc,
            "per_exit_inline entry's cont_pc mismatch with IR"
        );
        DecodedExit {
            cont_pc,
            site_id,
            exit_hit_idx: idx,
            exit_tags_for_pc: &per_exit_inline[idx].exit_tags,
            using_global_exit_tags: false,
        }
    } else {
        match per_exit_tags
            .iter()
            .enumerate()
            .find(|(_, (pc, _))| *pc == cont_pc)
        {
            Some((i, (_, tags))) => DecodedExit {
                cont_pc,
                site_id: 0,
                exit_hit_idx: inline_n + i,
                exit_tags_for_pc: &**tags,
                using_global_exit_tags: false,
            },
            None => DecodedExit {
                cont_pc,
                site_id: 0,
                exit_hit_idx: inline_n + per_exit_tags.len(),
                exit_tags_for_pc: exit_tags,
                using_global_exit_tags: true,
            },
        }
    }
}

/// Compile-time options for the trace lowerer.
#[derive(Clone, Copy, Debug)]
pub struct CompileOptions {
    /// When `true`, the trace's clean-close path emits a back-edge
    /// jump to its own body-loop block instead of returning
    /// `head_pc` to the caller — so the JIT'd code runs in a tight
    /// native loop until a cmp side-exit fires. The dispatcher's
    /// per-entry marshal cost amortizes across however many
    /// iterations the trace runs before diverging.
    ///
    /// Internal-loop traces require at least one exit edge
    /// (`Lt / Le / Eq` cmp or `Op::ForLoop`) AND no `Op::Call`
    /// truncation — otherwise the trace would run forever. The
    /// lowerer auto-downgrades to one-shot when neither condition
    /// holds, so callers can safely set this `true` for any
    /// record.
    ///
    /// Defaults to `false` in `try_compile_trace` (one-shot, the
    /// shape unit tests assume) and `true` in
    /// `try_compile_trace_with_options` when callers explicitly
    /// want the dispatcher fast path.
    pub internal_loop: bool,
    /// Lua dialect — `true` for 5.1 / 5.2 / 5.3, `false` for
    /// 5.4 / 5.5. The numeric `for` op (`Op::ForLoop`) has a
    /// different layout pre-5.3 (the slot at `R[A+1]` is the raw
    /// `limit` Value, not a remaining-iteration count). Step-6
    /// only lowers the 5.4+ Int count form; pre-5.3 traces bail
    /// and stay on the interp side.
    pub pre53: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            internal_loop: false,
            pre53: false,
        }
    }
}
