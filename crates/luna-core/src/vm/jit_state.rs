//! `JitState` sidecar: JIT-specific Vm state factored out
//! of the [`crate::vm::Vm`] struct.
//!
//! The interpreter dispatch loop reads `self.heap`,
//! `self.stack`, `self.frames`, ... as inherent fields; JIT state
//! lives one field hop away (`self.jit.active_trace` instead of
//! `self.active_trace`). The goal is physical separation between
//! interp and JIT bookkeeping at the field level.
//!
//! `JitState` is always present on a Vm — even an embedder that
//! never runs JIT'd code holds an inert `JitState` whose
//! `chunk_compiler` / `trace_compiler` are
//! [`crate::jit::NullJitBackend`]. The dispatcher reads
//! `self.jit.chunk_compiler` on every JIT entry, so the indirection
//! is fixed; `Option<JitState>` would impose an `unwrap()` on the
//! hot path for no benefit.
//!
//! Visibility: `#[doc(hidden)] pub` mirrors the existing pattern
//! used by the `#[doc(hidden)] pub fn jit_*` Vm helper methods.
//! Cross-crate access from `luna::jit_backend::*`
//! (which writes `vm.jit.pending_err` from `extern "C"` Cranelift
//! helpers) requires the struct + field to be `pub` somewhere
//! reachable, and `#[doc(hidden)]` keeps it out of the public
//! rustdoc surface.

use crate::runtime::Value;
use crate::vm::error::LuaError;

/// JIT-specific Vm state. See module docs.
#[doc(hidden)]
pub struct JitState {
    /// Master JIT switch. Off until a real backend is installed
    /// ([`crate::vm::Vm::install_jit_backend`]), which turns it on unless
    /// the embedder already chose a value with `Vm::set_jit_enabled`.
    /// Compiled code does not tick `instr_budget`, so while a budget or
    /// a memory cap is armed none is entered (`Vm::limited`), whatever
    /// this says.
    pub enabled: bool,

    /// Trace JIT subswitch. Same default and install rule as
    /// [`Self::enabled`].
    pub trace_enabled: bool,

    /// The embedder set [`Self::enabled`] / [`Self::trace_enabled`]
    /// explicitly; installing a backend then leaves that flag alone.
    pub(crate) enabled_chosen: bool,
    pub(crate) trace_enabled_chosen: bool,

    /// Back-edge visits before a loop is recorded as a trace, and calls
    /// before a function is. [`crate::jit::trace::TRACE_HOT_THRESHOLD`]
    /// and [`crate::jit::trace::CALL_HOT_THRESHOLD`] by default; tests
    /// lower them so that short programs exercise the trace JIT.
    pub trace_hot_threshold: u32,
    /// See [`Self::trace_hot_threshold`].
    pub call_hot_threshold: u32,
    /// The code generator traces are compiled with; `LUNA_TRACE_TIER`
    /// (`auto`, `baseline` or `optimizing`) sets the default.
    pub trace_tier: crate::jit::trace::TraceTier,
    /// [`crate::jit::trace::CompileOptions::tier_up_at`] for this Vm's
    /// traces.
    pub tier_up_at: u32,
    /// Ask the trace compiler for traces other Vms compiled before
    /// recording one (see [`crate::vm::Vm::enable_trace_sharing`]).
    pub(crate) share_traces: bool,

    /// Back-edge counts per loop head, indexed by a hash of the head's
    /// pc and its function's first line (LuaJIT's `hotcount`): each loop
    /// of a function gets hot on its own. Two heads sharing a slot make
    /// each other hot sooner, which changes where recording starts; the
    /// hash uses no address so that this is the same on every run.
    pub(crate) loop_hot: Box<[u32; LOOP_HOT_SLOTS]>,

    /// Opt-in flag for the self-link cycle catch. Default `false`:
    /// the catch has a known correctness problem, so it ships disabled.
    pub self_link_enabled: bool,

    /// Whether the recorder captures a table-field inline cache
    /// snapshot (see [`crate::jit::trace_types::field_ic_enabled`]).
    /// Starts from `LUNA_JIT_FIELD_IC`; the lowering only emits the
    /// cache for a trace whose record carries the snapshot.
    pub(crate) field_ic_enabled: bool,

    /// The trace currently being recorded, or `None` if
    /// the dispatch loop is in normal interpretation mode.
    pub active_trace: Option<Box<crate::jit::trace::TraceRecord>>,

    /// Index into `Vm.frames` of the Lua frame that the
    /// recorder started in.
    pub recording_frame_base: usize,

    /// Running max of `inline_depth` observed on
    /// any `RecordedOp` pushed by the recorder.
    pub max_depth_seen: u8,

    /// Diagnostic counters; see [`JitCounters`].
    pub counters: JitCounters,

    /// JIT-side error inbox set by a JIT table helper
    /// when it detects a metatable on the target table. Taken by
    /// the dispatcher after the JIT entry returns; the interp path
    /// re-executes the call with proper `__index`/`__newindex`
    /// semantics. Always `None` outside a JIT entry window.
    /// Written from `luna::jit_backend::*` Cranelift helpers.
    pub pending_err: Option<LuaError>,

    /// Reusable buffer for the trace JIT dispatcher's
    /// per-entry `reg_state`.
    pub reg_state_buf: Vec<i64>,

    /// Values compiled code holds only in machine registers while it calls
    /// a helper that can collect (a trace's concat or generic-for call). The
    /// helper pushes them before the call and truncates back after it, so
    /// nested entries stack; the collector treats them as roots.
    pub ssa_roots: Vec<Value>,

    /// Pool of reusable per-trace string accumulator
    /// buffers.
    pub str_buf_pool: Vec<Vec<u8>>,

    /// Cap on the buffer pool size.
    pub str_buf_pool_cap: usize,

    /// Companion buffer for `entry_tags` (one u8 per
    /// register at trace dispatch entry).
    pub entry_tags_buf: Vec<u8>,

    /// Closure-compile backend the dispatcher
    /// routes through. Default is [`crate::jit::NullJitBackend`];
    /// `Vm::install_jit_backend` swaps in caller-supplied
    /// backends (the `luna` crate installs `CraneliftBackend`).
    pub chunk_compiler: Box<dyn crate::jit::IntChunkCompiler>,

    /// Trace-JIT backend.
    pub trace_compiler: Box<dyn crate::jit::TraceCompiler>,

    /// Bounded stitch-back depth remaining for
    /// the dispatcher's `is_downrec_sentinel` admit path. Cycle-
    /// safety checkpoint: a `downrec_link`-bearing
    /// trace whose stitch target is itself can in principle keep
    /// returning the DOWNREC sentinel forever, and the dispatcher
    /// would forever re-admit it on the next interpreter loop
    /// iteration. The counter is consulted BEFORE every downrec
    /// admit; each admit decrements; when it would reach a negative
    /// value the dispatcher refuses entry and force-deopts via
    /// [`Self::suppress_downrec_admit_once`]. Reset to
    /// [`JitState::STITCH_DEPTH_DEFAULT`] each natural deopt or
    /// when the suppress flag fires (so a subsequent interp tick
    /// past `head_pc` re-arms the budget). Default = the constant.
    pub stitch_depth_remaining: u32,

    /// One-shot suppression flag for the
    /// dispatcher's trace admit. Set when a trace hands control back
    /// at its own `head_pc` without having run the op there: the
    /// dispatcher when it force-deopts a downrec entry (guard miss OR
    /// cycle-budget exhausted), and a trace side exit taken before the
    /// head op (through `luna_jit_suppress_trace_admit`). The NEXT
    /// interpreter loop iteration skips the admit and lets interp run
    /// the op at `head_pc`, advancing `pc` past `head_pc` and breaking
    /// the otherwise-infinite admit loop. Consumed (cleared) the first
    /// time the dispatcher reads it.
    pub suppress_downrec_admit_once: bool,

    /// Per-`Vm` JIT storage holder.
    /// Default is [`crate::jit::NullJitStorage`]; the `luna_jit`
    /// crate's `install_default_jit` swaps in a
    /// `CraneliftJitStorage` carrying the cache + compiled-handle
    /// collections. Accessed via downcast
    /// from the `CraneliftBackend` trait impls.
    pub storage: Box<dyn crate::jit::JitStorage>,

    /// An error raised by a call that compiled code handed to the
    /// interpreter (a self-recursive call made with too little native
    /// stack left): unlike `pending_err` it is the call's outcome, raised
    /// where the compiled code was entered, not a reason to run it again.
    pub pending_raise: Option<LuaError>,
}

impl JitState {
    /// Default per-dispatch stitch-back depth. The downrec admit
    /// is gated by a multi-way CMP-chain that is a real runtime
    /// guard (not constant-folded), so the only
    /// way a downrec trace HITs is when `saved_pc` from the parent
    /// frame matches one of the recorded `caller_pc` candidates;
    /// each natural admit corresponds to ONE Lua call chain pop, so
    /// the budget can safely grow to cover ~all consecutive HITs
    /// expected in a hot loop without infinite-loop risk. `32` lets
    /// 31 HITs accumulate before a forced-deopt resets the budget;
    /// fib(3) hot loop's per-outer-iter pattern shows 1 HIT every
    /// 5 admits, so `32` covers ~32 outer iters before any
    /// false-classify pressure.
    pub const STITCH_DEPTH_DEFAULT: u32 = 32;
}

/// Diagnostic counters and probe lists. All fields here are
/// diagnostic-only — they never affect dispatch correctness, and
/// can be cleared/snapshotted as a unit by tests.
#[doc(hidden)]
#[derive(Default)]
pub struct JitCounters {
    /// Number of traces that have closed cleanly.
    pub closed: u64,
    /// Traces moved to the optimizing tier.
    pub tiered_up: u64,
    /// Number of traces that have aborted.
    pub aborted: u64,
    /// Number of compiled traces that closed at a
    /// `TraceEnd::InlineAbort` exit.
    pub inline_abort: u64,
    /// Count of closed traces the lowerer compiled.
    pub compiled: u64,
    /// Traces installed from code another Vm compiled, without compiling.
    pub adopted: u64,
    /// Count of closed traces the lowerer rejected.
    pub compile_failed: u64,
    /// Number of trace dispatch entries.
    pub dispatched: u64,
    /// Number of trace entries that came back with
    /// `jit_pending_err` set.
    pub deopt: u64,
    /// Count of side-trace recordings the dispatcher
    /// started.
    pub side_trace_started: u64,
    /// Count of side-trace recordings that closed
    /// AND reached the lowerer with a non-None outcome.
    pub side_trace_compiled: u64,
    /// Count of side traces that compiled but
    /// failed the shape-match gate.
    pub side_trace_shape_mismatch: u64,
    /// Dispatches of traces holding each kind of inlined code, by the bit
    /// of [`crate::jit::trace::CompiledTrace::inline_kinds`].
    pub inline_kind_dispatched: [u64; 4],
    /// Recordings not compiled because other Vms sharing compiled code
    /// failed to compile one like it.
    pub shared_failures_known: u64,
    /// The failures of other Vms counted as this one's at those
    /// recordings.
    pub shared_failures_counted: u64,
    /// Recordings compiled only up to a call they had followed into a
    /// function the trace could not hold.
    pub inline_cut: u64,
    /// Runs of a side trace from its parent's exit.
    pub side_trace_runs: u64,
    /// Of `side_trace_runs`, those from an exit inside a function the
    /// parent inlined.
    pub side_trace_runs_inlined: u64,
    /// Tally of NewTable sites flagged Sinkable.
    pub sinkable_seen: u64,
    /// Cumulative count of `BufferState::Bufferable`
    /// accumulator sites.
    pub accum_bufferable_seen: u64,
    /// Tally of Sinkable sites that took the sunk-emit
    /// path.
    pub sunk_alloc: u64,
    /// Tally of materialise-helper emit sites.
    pub materialize_emit: u64,
    /// Number of compiled
    /// traces whose `CompiledTrace.per_exit_inline.len() > 0` (depth>0
    /// inlined cmp side-exits were emitted). Probed via
    /// `Vm::trace_per_exit_inline_compiled_count`. Together with
    /// `per_exit_inline_dispatchable`, lets a diag distinguish
    /// "recorder + lowerer can produce inline side-exits" from
    /// "compiled trace is dispatchable enough to exercise the AOT
    /// inline-chain reloc + deploy-resolver path".
    pub per_exit_inline_compiled: u64,
    /// Subset of
    /// `per_exit_inline_compiled` that ALSO has `dispatchable == true`.
    /// This is the count of traces that would actually exercise the
    /// AOT inline-chain reloc + deploy-resolver path. Probed
    /// via `Vm::trace_per_exit_inline_dispatchable_count`.
    pub per_exit_inline_dispatchable: u64,
    /// Total `Op::Closure` ops the trace JIT lowered to
    /// `luna_jit_op_closure` helper calls.
    pub closure_emit: u64,
    /// Every compiled trace's `dispatch_off_reason`
    /// pushed at compile time.
    pub dispatch_off_reasons: Vec<&'static str>,
    /// Every `try_compile_trace_with_options` None
    /// return's last checkpoint.
    pub compile_failed_reasons: Vec<&'static str>,
    /// Every closed trace's `(is_call_triggered, ops_len)`.
    pub closed_lens: Vec<(bool, usize)>,
    /// Close-cause counts. Single per-reason bucket
    /// that covers BOTH recorder-side abort/discard outcomes AND
    /// lowerer-side dispatch_off (`dispatchable=false` post-compile)
    /// outcomes, so probes can answer "how many of each reason fired"
    /// in O(1); `aborted`, `closed_lens` and `dispatch_off_reasons`
    /// carry no per-reason count.
    ///
    /// Labels currently bumped (see `bump_close_cause` callers):
    /// - `"trace-overflow"` (recorder MAX_TRACE_LEN overflow)
    /// - `"partial-coverage-discard"` (recorder cap-not-reached discard)
    /// - `"self-link-retf-r1"` (lowerer self-link dispatchable=false)
    /// - `"selflink-yields-to-downrec"` (recorder self-link
    ///   trip rerouted to `downrec_close` when `cur_depth >= 2` AND a
    ///   parent `Op::Call` ancestor exists in `rec.ops`; moves fib(28)-
    ///   like shapes onto the DownRec lowerer arm — a single-candidate
    ///   guard chain keeps dispatchable=false + the
    ///   `"downrec-stitch-pending"` label)
    /// - `"length-gate"` / `"InlineAbort-gate"` / `"GetI:inference-fail"`
    ///   / `"GetTable:inference-fail"` / `"GetField:inference-fail"`
    ///   / `"GetTabUp:inference-fail"` / `"GetUpval:not-Closure-use"`
    ///   (every lowerer-side dispatch_off label that already exists
    ///   on `CompiledTrace.dispatch_off_reason`)
    pub close_cause_counts: std::collections::HashMap<&'static str, u64>,
    /// Number of times the trace recorder captured
    /// a [`crate::jit::trace_types::FieldIcSnapshot`] for the first
    /// eligible `Op::GetField` site with the field IC switch on.
    /// Bumped exactly once per recording (the snapshot field is
    /// `Option<_>` so subsequent GetFields short-circuit). 0 on the
    /// env-default path.
    pub field_ic_snapshot_captured: u64,
    /// Number of compiled traces whose
    /// `CompiledTrace.downrec_link` is `Some(_)`. Bumped at trace
    /// finalisation alongside the `dispatch_off_reasons.push` site
    /// (`exec.rs` close handler) when the lowerer's
    /// `downrec_idx_opt` arm emitted the stitch sentinel + caller-pc
    /// guard scaffold.
    pub downrec_link_compiled: u64,
    /// Number of times the dispatcher's
    /// `is_downrec_sentinel` arm in
    /// `crates/luna-core/src/vm/exec.rs` fired with the caller-pc
    /// guard reporting a HIT (saved-PC at `reg_state[window_size]`
    /// matched the recorded `dr_return_pc`). Each bump corresponds
    /// to one stitch-back round: the trace returned the
    /// `SIDE_SENT_DOWNREC_CODE` sentinel and the dispatcher fed the
    /// trace's `head_pc` back to the interpreter loop so the
    /// admit-by-`downrec_link` gate re-enters the trace (bounded by
    /// the dispatcher's `stitch_depth_remaining` checkpoint).
    pub downrec_dispatched: u64,
    /// Number of times the dispatcher's
    /// `is_downrec_sentinel` arm observed a guard MISS (the trace
    /// invocation returned with `downrec_link.is_some()` but the
    /// returned sentinel was NOT [`SIDE_SENT_DOWNREC_CODE`] — i.e.
    /// the lowerer's `deopt_blk` arm fired, returning `head_pc` via
    /// the GLOBAL sentinel). Bumped on the dispatcher side via the
    /// post-invoke check so the caller-pc guard miss-rate can be
    /// measured via `downrec_dispatched + downrec_deopt`.
    pub downrec_deopt: u64,
    /// Number of compiled traces whose
    /// `CompiledTrace.downrec_multi_way_count >= 2`. Bumped at the
    /// close handler in `crates/luna-core/src/vm/exec.rs` alongside
    /// `downrec_link_compiled`. A single-CMP guard never bumps this
    /// counter. Independent of the dispatcher's `downrec_dispatched` /
    /// `downrec_deopt` counters, which measure runtime guard hit-rate.
    pub multi_way_guard_emitted: u64,
}

impl JitState {
    /// [`crate::jit::trace::TraceRecord::settings`] for a recording now.
    pub(crate) fn recording_settings(&self) -> u8 {
        u8::from(self.field_ic_enabled) | (u8::from(self.self_link_enabled) << 1)
    }
}

impl JitCounters {
    /// Bump the close-cause bucket for `reason`.
    /// Mirrors the existing per-site pattern (`aborted += 1`,
    /// `dispatch_off_reasons.push(reason)`) but with O(1) per-reason
    /// access via a `HashMap`. Single source of truth for the
    /// close-cause taxonomy probe surface
    /// (`Vm::trace_close_cause_counts`).
    #[inline]
    pub fn bump_close_cause(&mut self, reason: &'static str) {
        *self.close_cause_counts.entry(reason).or_insert(0) += 1;
    }
}

/// Slots in [`JitState::loop_hot`].
const LOOP_HOT_SLOTS: usize = 256;

impl JitState {
    /// Count a back-edge to the loop head at pc `head` of `proto`; `true`
    /// once that head has been crossed more than
    /// [`Self::trace_hot_threshold`] times, which starts its count over.
    #[inline]
    pub(crate) fn loop_hot_tick(&mut self, proto: &crate::runtime::Proto, head: u32) -> bool {
        let key = proto
            .line_defined
            .wrapping_mul(0x9e37_79b1)
            .wrapping_add(head);
        let slot = &mut self.loop_hot[key as usize & (LOOP_HOT_SLOTS - 1)];
        if *slot >= self.trace_hot_threshold {
            *slot = 0;
            true
        } else {
            *slot += 1;
            false
        }
    }

    /// Build an inert `JitState` whose backends are
    /// [`crate::jit::NullJitBackend`], with `enabled` and
    /// `trace_enabled` off: nothing could compile, so the interpreter
    /// skips the hot counters, the recorder and the trace lookup.
    /// `Vm::new_inner` calls this; the `luna` crate's
    /// `Vm::new_minimal_with_jit` then swaps the backends to
    /// `CraneliftBackend` via `Vm::install_jit_backend`, which turns
    /// both flags on.
    pub fn with_null_backend() -> JitState {
        JitState {
            enabled: false,
            trace_enabled: false,
            enabled_chosen: false,
            trace_enabled_chosen: false,
            trace_hot_threshold: crate::jit::trace::TRACE_HOT_THRESHOLD,
            call_hot_threshold: crate::jit::trace::CALL_HOT_THRESHOLD,
            trace_tier: default_trace_tier(),
            tier_up_at: crate::jit::trace::TIER_UP_THRESHOLD,
            share_traces: false,
            loop_hot: Box::new([0; LOOP_HOT_SLOTS]),
            self_link_enabled: false,
            field_ic_enabled: crate::jit::trace_types::field_ic_enabled(),
            active_trace: None,
            recording_frame_base: 0,
            max_depth_seen: 0,
            counters: JitCounters::default(),
            pending_err: None,
            pending_raise: None,
            reg_state_buf: Vec::new(),
            ssa_roots: Vec::new(),
            str_buf_pool: Vec::new(),
            str_buf_pool_cap: 4,
            entry_tags_buf: Vec::new(),
            chunk_compiler: Box::new(crate::jit::NullJitBackend),
            trace_compiler: Box::new(crate::jit::NullJitBackend),
            stitch_depth_remaining: JitState::STITCH_DEPTH_DEFAULT,
            suppress_downrec_admit_once: false,
            storage: Box::new(crate::jit::NullJitStorage),
        }
    }
}

fn default_trace_tier() -> crate::jit::trace::TraceTier {
    use crate::jit::trace::TraceTier;
    static TIER: std::sync::OnceLock<TraceTier> = std::sync::OnceLock::new();
    *TIER.get_or_init(|| match std::env::var("LUNA_TRACE_TIER").as_deref() {
        Ok("baseline") => TraceTier::Baseline,
        Ok("optimizing") => TraceTier::Optimizing,
        _ => TraceTier::Auto,
    })
}
