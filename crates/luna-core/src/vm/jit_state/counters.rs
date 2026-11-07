//! The JIT's diagnostic counters.

/// Diagnostic counters and probe lists: they never affect dispatch, and
/// tests clear or snapshot them as a unit.
#[derive(Default)]
pub struct JitCounters {
    /// Number of traces that have closed cleanly.
    pub closed: u64,
    /// Traces moved to the optimizing tier.
    pub tiered_up: u64,
    /// Traces at the optimizing tier compiled again: one of their side
    /// cells got a side trace their code did not call.
    pub retiered: u64,
    /// Functions the method JIT compiled.
    pub method_compiled: u64,
    /// Functions whose method JIT code a backend replaced in the background (`Proto::jit_next`).
    pub method_replaced: u64,
    /// Number of traces that have aborted.
    pub aborted: u64,
    /// Trace heads no longer recorded: their recordings failed or aborted too often.
    pub blacklisted: u64,
    /// Compiled traces that closed at a `TraceEnd::InlineAbort` exit.
    pub inline_abort: u64,
    /// Count of closed traces the lowerer compiled.
    pub compiled: u64,
    /// Traces installed from code another Vm compiled, without compiling.
    pub adopted: u64,
    /// Count of closed traces the lowerer rejected.
    pub compile_failed: u64,
    /// Traces compiled or adopted but not installed: the Vm had no memory
    /// for what running them needs. Not a failure of their head, which is
    /// recorded again once it is hot.
    pub install_no_room: u64,
    /// Number of trace dispatch entries.
    pub dispatched: u64,
    /// Number of trace entries that came back with `jit_pending_err` set.
    pub deopt: u64,
    /// Count of side-trace recordings the dispatcher started.
    pub side_trace_started: u64,
    /// Side-trace recordings that closed and the lowerer compiled.
    pub side_trace_compiled: u64,
    /// Side traces that compiled but failed the shape-match gate.
    pub side_trace_shape_mismatch: u64,
    /// Side traces wired to an exit, by what became of the exit's side
    /// cell: the parent's code runs them (0), or not because the exit has
    /// no cell (1), the side trace is wider than the parent (2), ends at
    /// the head of a trace other than the parent where no trace of its
    /// frame starts (3), the exit resumes in another frame (4),
    /// or the parent (or the trace it links to) may not be entered with
    /// what it leaves (5), or the parent's code runs them and enters the
    /// other trace of the frame they end at (6), or not because the side
    /// trace ends at no trace's head (7). A cell left empty for want
    /// of a trace at the head the side trace ends at moves from 3 once one
    /// is cached there.
    pub relink: [u64; 8],
    /// The times compiled code entered the trace a side trace links to
    /// without the dispatcher.
    pub links: u64,
    /// Of `links`, the ones the dispatcher took after the code left them,
    /// the links in a row at their limit.
    pub links_redispatched: u64,
    /// Dispatches of traces holding each kind of inlined code, by the bit
    /// of [`crate::jit::trace::CompiledTraceParts::inline_kinds`].
    pub inline_kind_dispatched: [u64; 4],
    /// Recordings not compiled because other Vms sharing compiled code
    /// failed to compile one like it.
    pub shared_failures_known: u64,
    /// The failures of other Vms counted as this one's at those recordings.
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
    /// Cumulative count of `BufferState::Bufferable` accumulator sites.
    pub accum_bufferable_seen: u64,
    /// Tally of Sinkable sites that took the sunk-emit path.
    pub sunk_alloc: u64,
    /// Tally of materialise-helper emit sites.
    pub materialize_emit: u64,
    /// Compiled traces with depth>0 inlined cmp side-exits (`per_exit_inline` not empty).
    /// With `per_exit_inline_dispatchable`, it tells "the recorder and lowerer can produce
    /// inline side-exits" from "a trace is dispatchable enough to exercise the AOT
    /// inline-chain reloc and deploy-resolver path".
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
    /// Number of times a down-recursion trace went back to the
    /// interpreter instead of stitching: its stitch return came after
    /// the `stitch_depth_remaining` budget was spent, or a helper it
    /// called raised. A caller-pc guard miss leaves through the
    /// lowerer's `deopt_blk` as a plain exit and is not counted here.
    pub downrec_deopt: u64,
    /// Number of compiled traces whose
    /// `CompiledTrace.downrec_multi_way_count >= 2`. Bumped at the
    /// close handler in `crates/luna-core/src/vm/exec.rs` alongside
    /// `downrec_link_compiled`. A single-CMP guard never bumps this
    /// counter. Independent of the dispatcher's `downrec_dispatched` /
    /// `downrec_deopt` counters, which measure runtime guard hit-rate.
    pub multi_way_guard_emitted: u64,
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
