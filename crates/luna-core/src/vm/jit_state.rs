//! v1.1 A2 — `JitState` sidecar: JIT-specific Vm state factored out
//! of the [`crate::vm::Vm`] struct.
//!
//! See `.dev/rfcs/v1.1-rfc-vm-jitstate-split.md` for the design
//! rationale. The interpreter dispatch loop reads `self.heap`,
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
//! used by the 18 `#[doc(hidden)] pub fn jit_*` Vm helper methods
//! (Session C). Cross-crate access from `luna::jit_backend::*`
//! (which writes `vm.jit.pending_err` from `extern "C"` Cranelift
//! helpers) requires the struct + field to be `pub` somewhere
//! reachable, and `#[doc(hidden)]` keeps it out of the public
//! rustdoc surface.

use crate::vm::error::LuaError;

/// JIT-specific Vm state. See module docs.
#[doc(hidden)]
pub struct JitState {
    /// Master JIT switch (was `Vm::jit_enabled`). Default `true`.
    /// Sandbox embedders that rely on `instr_budget` for DoS
    /// containment **must** call `Vm::set_jit_enabled(false)` —
    /// JIT'd counted-for loops compile to native Cranelift IR
    /// that does not tick the budget.
    pub enabled: bool,

    /// P12-S1 — trace JIT subswitch (was `Vm::trace_jit_enabled`).
    /// `false` by default so existing benchmarks see zero overhead
    /// while the sprint develops.
    pub trace_enabled: bool,

    /// P16-A — opt-in flag for the self-link cycle catch (was
    /// `Vm::p16_self_link_enabled`). Default `false` —
    /// SHIPPED-DISABLED in v1.0 due to P16-B correctness blocker.
    pub p16_self_link_enabled: bool,

    /// P12-S1 — the trace currently being recorded, or `None` if
    /// the dispatch loop is in normal interpretation mode. (Was
    /// `Vm::active_trace`.)
    pub active_trace: Option<Box<crate::jit::trace::TraceRecord>>,

    /// P12-S4 — index into `Vm.frames` of the Lua frame that the
    /// recorder started in. (Was `Vm::recording_frame_base`.)
    pub recording_frame_base: usize,

    /// P12-S4-step1 — running max of `inline_depth` observed on
    /// any `RecordedOp` pushed by the recorder. (Was
    /// `Vm::trace_max_depth_seen`.)
    pub max_depth_seen: u8,

    /// Diagnostic counters; see [`JitCounters`].
    pub counters: JitCounters,

    /// P11-S5d.E' — JIT-side error inbox set by a JIT table helper
    /// when it detects a metatable on the target table. Taken by
    /// the dispatcher after the JIT entry returns; the interp path
    /// re-executes the call with proper `__index`/`__newindex`
    /// semantics. Always `None` outside a JIT entry window.
    /// (Was `Vm::jit_pending_err` — the one `pub` field on Vm
    /// reached from `luna::jit_backend::*` Cranelift helpers.)
    pub pending_err: Option<LuaError>,

    /// P13-S13-D — reusable buffer for the trace JIT dispatcher's
    /// per-entry `reg_state`. (Was `Vm::jit_reg_state_buf`.)
    pub reg_state_buf: Vec<i64>,

    /// P14-S14-B v2 — pool of reusable per-trace string accumulator
    /// buffers. (Was `Vm::jit_str_buf_pool`.)
    pub str_buf_pool: Vec<Vec<u8>>,

    /// P14-S14-B v2 — cap on the buffer pool size. (Was
    /// `Vm::jit_str_buf_pool_cap`.)
    pub str_buf_pool_cap: usize,

    /// P13-S13-D — companion buffer for `entry_tags` (one u8 per
    /// register at trace dispatch entry). (Was
    /// `Vm::jit_entry_tags_buf`.)
    pub entry_tags_buf: Vec<u8>,

    /// v1.1 A1 Session A — closure-compile backend the dispatcher
    /// routes through. Default is [`crate::jit::NullJitBackend`];
    /// `Vm::install_jit_backend` swaps in caller-supplied
    /// backends (the `luna` crate installs `CraneliftBackend`).
    /// (Was `Vm::chunk_compiler`.)
    pub chunk_compiler: Box<dyn crate::jit::IntChunkCompiler>,

    /// v1.1 A1 Session A — trace-JIT backend. (Was
    /// `Vm::trace_compiler`.)
    pub trace_compiler: Box<dyn crate::jit::TraceCompiler>,
}

/// Diagnostic counters and probe lists. All fields here are
/// diagnostic-only — they never affect dispatch correctness, and
/// can be cleared/snapshotted as a unit by tests.
#[doc(hidden)]
#[derive(Default)]
pub struct JitCounters {
    /// P12-S1.D — number of traces that have closed cleanly. (Was
    /// `Vm::trace_closed_count`.)
    pub closed: u64,
    /// P12-S1.D — number of traces that have aborted. (Was
    /// `Vm::trace_aborted_count`.)
    pub aborted: u64,
    /// P13-S13-G v2 — number of compiled traces that closed at a
    /// `TraceEnd::InlineAbort` exit. (Was
    /// `Vm::trace_inline_abort_count`.)
    pub inline_abort: u64,
    /// P12-S2.C — count of closed traces the lowerer compiled.
    /// (Was `Vm::trace_compiled_count`.)
    pub compiled: u64,
    /// P12-S2.C — count of closed traces the lowerer rejected.
    /// (Was `Vm::trace_compile_failed_count`.)
    pub compile_failed: u64,
    /// P12-S3 — number of trace dispatch entries. (Was
    /// `Vm::trace_dispatched_count`.)
    pub dispatched: u64,
    /// P12-S3 — number of trace entries that came back with
    /// `jit_pending_err` set. (Was `Vm::trace_deopt_count`.)
    pub deopt: u64,
    /// P15-A v1 — count of side-trace recordings the dispatcher
    /// started. (Was `Vm::trace_side_trace_started_count`.)
    pub side_trace_started: u64,
    /// P15-A v2-A — count of side-trace recordings that closed
    /// AND reached the lowerer with a non-None outcome. (Was
    /// `Vm::trace_side_trace_compiled_count`.)
    pub side_trace_compiled: u64,
    /// P15-A v2-C-A5-C — count of side traces that compiled but
    /// failed the shape-match gate. (Was
    /// `Vm::trace_side_trace_shape_mismatch_count`.)
    pub side_trace_shape_mismatch: u64,
    /// P12-S5-A — tally of NewTable sites flagged Sinkable. (Was
    /// `Vm::trace_sinkable_seen_count`.)
    pub sinkable_seen: u64,
    /// P14-S14-B v1 — cumulative count of `BufferState::Bufferable`
    /// accumulator sites. (Was `Vm::trace_accum_bufferable_seen_count`.)
    pub accum_bufferable_seen: u64,
    /// P12-S5-B — tally of Sinkable sites that took the sunk-emit
    /// path. (Was `Vm::trace_sunk_alloc_count`.)
    pub sunk_alloc: u64,
    /// P12-S5-C — tally of materialise-helper emit sites. (Was
    /// `Vm::trace_materialize_emit_count`.)
    pub materialize_emit: u64,
    /// P12-S7-A — total `Op::Closure` ops the trace JIT lowered to
    /// `luna_jit_op_closure` helper calls. (Was
    /// `Vm::trace_closure_emit_count`.)
    pub closure_emit: u64,
    /// P13-S13-G v2.5 — every compiled trace's `dispatch_off_reason`
    /// pushed at compile time. (Was `Vm::trace_dispatch_off_reasons`.)
    pub dispatch_off_reasons: Vec<&'static str>,
    /// P13-S13-G v2.6 — every `try_compile_trace_with_options` None
    /// return's last checkpoint. (Was
    /// `Vm::trace_compile_failed_reasons`.)
    pub compile_failed_reasons: Vec<&'static str>,
    /// P13-S13-H — every closed trace's `(is_call_triggered, ops_len)`.
    /// (Was `Vm::trace_closed_lens`.)
    pub closed_lens: Vec<(bool, usize)>,
    /// v2.0 Track-R R2 — close-cause hygiene. Single per-reason bucket
    /// that covers BOTH recorder-side abort/discard outcomes AND
    /// lowerer-side dispatch_off (`dispatchable=false` post-compile)
    /// outcomes. Pre-R2 the close-cause taxonomy was split across
    /// `aborted` (u64, no reason label), `closed_lens` (mixes real
    /// closes and partial-coverage discards), and
    /// `dispatch_off_reasons` (Vec ordered append, O(N) to count by
    /// reason). R2 lifts the four known recorder/lowerer close sites
    /// into this single HashMap via `bump_close_cause` so probes can
    /// answer "how many of each reason fired" in O(1).
    ///
    /// Labels currently bumped (see `bump_close_cause` callers):
    /// - `"trace-overflow"` (recorder MAX_TRACE_LEN overflow)
    /// - `"partial-coverage-discard"` (recorder S13-I cap-not-reached discard)
    /// - `"self-link-retf-r1"` (lowerer SelfLink-R1 dispatchable=false)
    /// - `"length-gate"` / `"InlineAbort-gate"` / `"GetI:inference-fail"`
    ///   / `"GetTable:inference-fail"` / `"GetField:inference-fail"`
    ///   / `"GetTabUp:inference-fail"` / `"GetUpval:not-Closure-use"`
    ///   (every lowerer-side dispatch_off label that already exists
    ///   on `CompiledTrace.dispatch_off_reason`)
    pub close_cause_counts: std::collections::HashMap<&'static str, u64>,
}

impl JitCounters {
    /// v2.0 Track-R R2 — bump the close-cause bucket for `reason`.
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

impl JitState {
    /// Build an inert `JitState` whose backends are
    /// [`crate::jit::NullJitBackend`]. `enabled = true` (preserves
    /// v1.0 surface behavior); **`trace_enabled = true`** (v1.3 TA3 flip
    /// after Phase P2A Path B math.min/max fold landed `trace_dispatched_count
    /// 0 → 200/200` on token_bucket and Linux taskset perf-gate confirmed
    /// `redis_lua_shape ≥ 1.0×` baseline). Embedders that want the
    /// v1.2 interp-only default call `vm.set_trace_jit_enabled(false)`
    /// explicitly.
    /// `Vm::new_inner` calls this; the `luna` crate's
    /// `Vm::new_minimal_with_jit` then swaps the backends to
    /// `CraneliftBackend` via `Vm::install_jit_backend`.
    pub fn with_null_backend() -> JitState {
        JitState {
            enabled: true,
            trace_enabled: true,
            p16_self_link_enabled: false,
            active_trace: None,
            recording_frame_base: 0,
            max_depth_seen: 0,
            counters: JitCounters::default(),
            pending_err: None,
            reg_state_buf: Vec::new(),
            str_buf_pool: Vec::new(),
            str_buf_pool_cap: 4,
            entry_tags_buf: Vec::new(),
            chunk_compiler: Box::new(crate::jit::NullJitBackend),
            trace_compiler: Box::new(crate::jit::NullJitBackend),
        }
    }
}
