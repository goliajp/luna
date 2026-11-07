//! Read-only trace JIT counters for tests and diagnostics.

mod hot_exits;

use super::*;

impl Vm {
    /// Traces that closed cleanly (looped back to the head PC) since the Vm was made.
    pub fn trace_closed_count(&self) -> u64 {
        self.jit.counters.closed
    }

    /// Traces that aborted (exceeded MAX_TRACE_LEN or hit an un-recordable op).
    pub fn trace_aborted_count(&self) -> u64 {
        self.jit.counters.aborted
    }

    /// Trace heads no longer recorded: their recordings failed or aborted too often.
    pub fn trace_blacklisted_count(&self) -> u64 {
        self.jit.counters.blacklisted
    }

    /// Compiled traces closed at a `TraceEnd::InlineAbort` (depth>0) boundary. They are not
    /// dispatchable: the dispatcher cannot resume at a depth>0 PC without the CallFrames.
    pub fn trace_inline_abort_count(&self) -> u64 {
        self.jit.counters.inline_abort
    }

    /// See `JitCounters::dispatch_off_reasons`.
    pub fn trace_dispatch_off_reasons(&self) -> &[&'static str] {
        &self.jit.counters.dispatch_off_reasons
    }

    /// See `JitCounters::compile_failed_reasons`.
    pub fn trace_compile_failed_reasons(&self) -> &[&'static str] {
        &self.jit.counters.compile_failed_reasons
    }

    /// See `JitCounters::closed_lens`. Returns
    /// `(is_call_triggered, ops_len)` for every trace that closed.
    pub fn trace_closed_lens(&self) -> &[(bool, usize)] {
        &self.jit.counters.closed_lens
    }

    /// See [`crate::vm::jit_state::JitCounters::close_cause_counts`].
    /// Per-reason close-cause counts (recorder-side abort/discard +
    /// lowerer-side dispatch_off labels) keyed by `&'static str`.
    pub fn trace_close_cause_counts(&self) -> &std::collections::HashMap<&'static str, u64> {
        &self.jit.counters.close_cause_counts
    }

    /// Number of compiled traces whose
    /// `CompiledTrace.downrec_link` is `Some(_)` (lowerer's
    /// `downrec_idx_opt` arm emitted the stitch sentinel + caller-pc
    /// guard scaffold).
    pub fn trace_downrec_link_compiled_count(&self) -> u64 {
        self.jit.counters.downrec_link_compiled
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::downrec_dispatched`]. Number
    /// of times the dispatcher's `is_downrec_sentinel` arm fired and
    /// classified the return as a caller-pc-guard HIT.
    pub fn trace_downrec_dispatched_count(&self) -> u64 {
        self.jit.counters.downrec_dispatched
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::downrec_deopt`]. Number of
    /// times a `downrec_link`-bearing trace went back to the
    /// interpreter at its stitch: a helper it called raised, or the
    /// dispatcher force-deopted via the stitch-cycle checkpoint (a
    /// caller-pc guard miss returns as a plain exit, not counted).
    pub fn trace_downrec_deopt_count(&self) -> u64 {
        self.jit.counters.downrec_deopt
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::multi_way_guard_emitted`].
    /// Number of compiled traces whose lowerer emitted a multi-way
    /// caller-pc guard chain (>= 2 distinct `caller_pc` candidates)
    /// at the `TraceEnd::DownRec` close + lifted `dispatchable = true`.
    pub fn trace_multi_way_guard_emitted_count(&self) -> u64 {
        self.jit.counters.multi_way_guard_emitted
    }

    /// Number of closed traces the lowerer compiled and
    /// parked on `Proto.traces`. Re-records of the same head_pc are
    /// deduped (the second close finds the head_pc already cached
    /// and skips compile), so this never exceeds `trace_closed_count`.
    pub fn trace_compiled_count(&self) -> u64 {
        self.jit.counters.compiled
    }

    /// Number of traces installed from code another Vm (or this one, for
    /// an earlier load of the same code) compiled, instead of being
    /// recorded and compiled here.
    pub fn trace_adopted_count(&self) -> u64 {
        self.jit.counters.adopted
    }

    /// Number of times the recorder captured a
    /// [`crate::jit::trace_types::FieldIcSnapshot`] with the field IC
    /// switch on ([`Self::set_field_ic_enabled`]). Stays 0 while it is
    /// off.
    pub fn trace_field_ic_snapshot_count(&self) -> u64 {
        self.jit.counters.field_ic_snapshot_captured
    }

    /// Number of closed traces the lowerer rejected
    /// (any of the bail conditions in
    /// `crate::jit::trace::try_compile_trace`).
    pub fn trace_compile_failed_count(&self) -> u64 {
        self.jit.counters.compile_failed
    }

    /// Number of times the dispatcher jumped into a
    /// compiled trace. Bumps on every entry; `trace_deopt_count`
    /// counts the subset where the trace returned with a parked
    /// `jit_pending_err`.
    pub fn trace_dispatched_count(&self) -> u64 {
        self.jit.counters.dispatched
    }

    /// Traces moved from the baseline to the optimizing tier.
    pub fn trace_tiered_up_count(&self) -> u64 {
        self.jit.counters.tiered_up
    }

    /// Number of trace entries that came back with
    /// `jit_pending_err` set (typically a metatable shadowed an
    /// index inside a helper, or compiled code could not allocate):
    /// the trace left at that op, which the interpreter runs.
    pub fn trace_deopt_count(&self) -> u64 {
        self.jit.counters.deopt
    }

    /// Number of times the dispatcher started a side
    /// trace recording (an `exit_hit_counts` slot crossed
    /// [`crate::jit::trace::HOTEXIT_THRESHOLD`] while `active_trace`
    /// was None and trace JIT was enabled). Each unit is exactly one
    /// `start_side_trace` call; the actual compile success counts
    /// under [`Self::trace_compiled_count`] like any other trace.
    /// Probe use: distinguishes the "side-trace pipeline fired"
    /// signal from the "primary back-edge / call-trigger fired"
    /// signal without reading per-counter histograms.
    pub fn trace_side_trace_started_count(&self) -> u64 {
        self.jit.counters.side_trace_started
    }

    /// Number of side-trace recordings that closed,
    /// compiled successfully, AND were wired to their parent's exit.
    pub fn trace_side_trace_compiled_count(&self) -> u64 {
        self.jit.counters.side_trace_compiled
    }

    /// Number of side traces that compiled
    /// successfully but were SHEDDED by the close-handler shape-
    /// match gate (`exit_tags_match_entry_tags`). High ratios
    /// vs. `trace_side_trace_compiled_count` indicate the
    /// architecture is shedding lots of would-be side traces;
    /// useful as a tuning probe for future relaxation of the
    /// gate or for child-IR re-specialisation against parent's
    /// exit shape.
    pub fn trace_side_trace_shape_mismatch_count(&self) -> u64 {
        self.jit.counters.side_trace_shape_mismatch
    }

    /// Dispatches of traces that hold inlined code of kind `kind` (one of
    /// the `INLINE_*` bits of [`crate::jit::trace::CompiledTraceParts::inline_kinds`]).
    pub fn trace_inline_kind_dispatched_count(&self, kind: u8) -> u64 {
        self.jit.counters.inline_kind_dispatched[kind.trailing_zeros() as usize & 3]
    }

    /// Recordings this Vm did not compile because other Vms sharing its
    /// engine failed to compile one like it (same code, start, entry tags
    /// and path).
    pub fn trace_shared_failures_known(&self) -> u64 {
        self.jit.counters.shared_failures_known
    }

    /// The failures of other Vms sharing this Vm's engine counted as this
    /// Vm's (see [`Self::trace_shared_failures_known`]).
    pub fn trace_shared_failures_counted(&self) -> u64 {
        self.jit.counters.shared_failures_counted
    }

    /// Recordings compiled only up to a call they had followed into a
    /// function the trace could not hold (the trace ends at that call).
    pub fn trace_inline_cut_count(&self) -> u64 {
        self.jit.counters.inline_cut
    }

    /// Runs of a side trace from its parent trace's exit.
    pub fn trace_side_trace_run_count(&self) -> u64 {
        self.jit.counters.side_trace_runs
    }

    /// Runs of a side trace from an exit inside a function its parent
    /// trace inlined (the side trace starts in that function's frame).
    pub fn trace_side_trace_inlined_run_count(&self) -> u64 {
        self.jit.counters.side_trace_runs_inlined
    }

    /// Sum of NewTable sites the pre-emit escape sweep
    /// classified as `crate::jit::trace::EscapeState::Sinkable`
    /// across every successfully compiled trace on this Vm. The
    /// count is post-demotion: sites pre-emit drops back to Escaped
    /// for not meeting the sunk-emit criteria are NOT counted.
    /// `trace_sunk_alloc_count` matches one-for-one today (every
    /// surviving Sinkable site goes through sunk emit).
    pub fn trace_sinkable_seen_count(&self) -> u64 {
        self.jit.counters.sinkable_seen
    }

    /// See `JitCounters::accum_bufferable_seen`.
    pub fn trace_accum_bufferable_seen_count(&self) -> u64 {
        self.jit.counters.accum_bufferable_seen
    }

    /// Total dispatch hits across all known traces,
    /// broken into hot-exit telemetry (max single-exit count,
    /// total dispatches, exit count). Used by probes to identify
    /// hot side-exits as side-trace candidates.
    ///
    /// Walks `cl.proto` AND all nested protos in `cl.proto.protos`
    /// recursively, so inner functions' traces are reported.
    pub fn trace_exit_hit_summary(
        &self,
        cl: crate::runtime::heap::Gc<crate::runtime::function::LuaClosure>,
    ) -> Vec<(u32, Vec<u32>)> {
        fn walk(
            proto: crate::runtime::heap::Gc<crate::runtime::function::Proto>,
            out: &mut Vec<(u32, Vec<u32>)>,
        ) {
            for ct in proto.traces.borrow().iter() {
                let counts: Vec<u32> = ct.exit_hit_counts.iter().map(|c| c.get()).collect();
                out.push((ct.head_pc, counts));
            }
            for inner in proto.protos.iter() {
                walk(*inner, out);
            }
        }
        let mut out: Vec<(u32, Vec<u32>)> = Vec::new();
        walk(cl.proto, &mut out);
        out
    }

    /// Sum of NewTable sites that actually took the
    /// sunk-emit path across every successfully compiled trace on
    /// this Vm. Each counted site skips its heap `Gc<Table>`
    /// allocation per dispatch; the array part lives as Cranelift
    /// `Variable`s for the duration of the trace.
    pub fn trace_sunk_alloc_count(&self) -> u64 {
        self.jit.counters.sunk_alloc
    }

    /// Sum of materialise-helper emit sites across every
    /// successfully compiled trace on this Vm. Each unit is a
    /// (site × cmp side-exit) pair whose IR reconstructs a heap
    /// `Gc<Table>` from the virt slots on deopt.
    pub fn trace_materialize_emit_count(&self) -> u64 {
        self.jit.counters.materialize_emit
    }

    /// Diagnostic: total `Op::Closure` ops the trace JIT
    /// lowered to the `luna_jit_op_closure` helper. Each emitted op
    /// replaces a `Heap::new_closure_inline` call on the dispatch
    /// path; the count is static (one per matching op per compiled
    /// trace), summed at compile success.
    pub fn trace_closure_emit_count(&self) -> u64 {
        self.jit.counters.closure_emit
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::per_exit_inline_compiled`].
    /// Number of compiled traces whose `per_exit_inline.len() > 0`
    /// (depth>0 inlined cmp side-exits emitted).
    pub fn trace_per_exit_inline_compiled_count(&self) -> u64 {
        self.jit.counters.per_exit_inline_compiled
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::per_exit_inline_dispatchable`].
    /// Number of compiled traces with `per_exit_inline.len() > 0` AND
    /// `dispatchable == true` — i.e. the count of compiled traces
    /// that would actually exercise the AOT chain-reloc +
    /// deploy-resolver path.
    pub fn trace_per_exit_inline_dispatchable_count(&self) -> u64 {
        self.jit.counters.per_exit_inline_dispatchable
    }

    /// Diagnostic: max `inline_depth` ever seen on any
    /// `RecordedOp` pushed by the recorder. Tells tests + tuning
    /// whether a self-recursive function actually walked the depth
    /// tracker past 0. Saturates at `MAX_INLINE_DEPTH`. Persists
    /// across traces and Vm activations; reset only on `Vm::new`.
    pub fn trace_max_depth_seen(&self) -> u8 {
        self.jit.max_depth_seen
    }
}
