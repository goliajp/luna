//! The recorder's view of a trace: recorded ops and the record being built.

use super::*;

mod op;
pub use op::*;

/// A recorded trace: a linear sequence of ops starting at a back-edge
/// target PC, terminating at either a loop close (back to head) or a
/// hard exit (return, error).
#[derive(Clone, Debug)]
pub struct TraceRecord {
    /// The PC the trace starts at (back-edge target).
    pub head_proto: Gc<Proto>,
    /// The recording Vm's switches that shape what is recorded (bit 0:
    /// field inline cache, bit 1: self-link): traces recorded under other
    /// switches are not shared.
    pub settings: u8,
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
    /// Generic-for iterator's library tag snapshot.
    /// Populated by `Op::TForLoop`'s recorder trigger when
    /// `R[A]` is `Value::Native`. Lets the lowerer specialise
    /// `Op::TForCall` emit on `ipairs_iter` (inline Table aget
    /// via `TABLE_ARRAY_PTR_OFFSET` / `TABLE_ASIZE_OFFSET` —
    /// skip the `luna_jit_op_tforcall` C call entirely). `None`
    /// for non-generic-for traces or when the recorder fires
    /// for a non-Native iter.
    pub tfor_iter: Option<crate::runtime::Builtin>,
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
    /// Per recorded op, for a `SelfOp` whose receiver lacked the key: the
    /// hash slot of `__index` in the receiver's metatable (high half) and
    /// of the key in that `__index` table (low half), or
    /// [`INDEX_SLOTS_UNKNOWN`]. The lowerer looks the method up there.
    pub index_slots: Vec<u64>,
    /// The interned `"__index"`, set when an op has `index_slots`.
    pub index_key: Option<Gc<crate::runtime::string::LuaStr>>,
    /// For the first numeric `for` loop's `ForLoop` at depth 0 (the one
    /// that ends the trace), when its step was an integer while it was
    /// recorded: whether that step was positive. The
    /// lowerer of a 5.3 integer loop checks the sign once before the loop
    /// instead of choosing the comparison on every iteration.
    pub for_step_up: Option<bool>,
    /// The recording closed because the interpreter came back to
    /// `head_pc` in the head frame: the op that would follow the last
    /// recorded one is the head.
    pub closed_at_head: bool,
    /// The recording ran a `Return` out of the frame it began in: an op
    /// recorded after that, short of returning past the head, is another
    /// call's (the host called in again).
    pub returned_from_head: bool,
}

/// [`TraceRecord::index_slots`] for an op with none.
pub const INDEX_SLOTS_UNKNOWN: u64 = u64::MAX;

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

    /// For `SelfOp` `i`: the slots of `__index` in the receiver's metatable
    /// and of the key in that table, if recorded.
    pub fn index_slots(&self, i: usize) -> Option<(u32, u32)> {
        self.index_slots
            .get(i)
            .copied()
            .filter(|&s| s != INDEX_SLOTS_UNKNOWN)
            .map(|s| ((s >> 32) as u32, s as u32))
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
            settings: 0,
            head_pc,
            entry_tags,
            ops: Vec::with_capacity(MAX_TRACE_LEN),
            closed: false,
            is_call_triggered,
            tfor_iter: None,
            tfor_val_tag: None,
            side_trace_parent: None,
            self_link_kind: None,
            retfs: Vec::new(),
            downrec_close: None,
            field_ic_snapshot: None,
            result_tags: Vec::with_capacity(MAX_TRACE_LEN),
            field_slots: Vec::with_capacity(MAX_TRACE_LEN),
            index_slots: Vec::with_capacity(MAX_TRACE_LEN),
            index_key: None,
            for_step_up: None,
            closed_at_head: false,
            returned_from_head: false,
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
            settings: 0,
            head_pc,
            entry_tags,
            ops: Vec::with_capacity(MAX_TRACE_LEN),
            closed: false,
            is_call_triggered: false,
            tfor_iter: None,
            tfor_val_tag: None,
            side_trace_parent: Some((parent_head_proto, parent_head_pc, parent_exit_idx)),
            self_link_kind: None,
            retfs: Vec::new(),
            downrec_close: None,
            field_ic_snapshot: None,
            result_tags: Vec::with_capacity(MAX_TRACE_LEN),
            field_slots: Vec::with_capacity(MAX_TRACE_LEN),
            index_slots: Vec::with_capacity(MAX_TRACE_LEN),
            index_key: None,
            for_step_up: None,
            closed_at_head: false,
            returned_from_head: false,
        }
    }

    /// The recording up to op `n` (exclusive): the ops after it, and what
    /// was noted about them, dropped.
    pub(crate) fn truncated(&self, n: usize) -> TraceRecord {
        let mut r = self.clone();
        r.closed_at_head &= n == r.ops.len();
        r.ops.truncate(n);
        r.result_tags.truncate(n);
        r.field_slots.truncate(n);
        r.index_slots.truncate(n);
        if r.field_ic_snapshot.is_some_and(|s| s.op_idx as usize >= n) {
            r.field_ic_snapshot = None;
        }
        r
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
        self.index_slots.push(INDEX_SLOTS_UNKNOWN);
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
