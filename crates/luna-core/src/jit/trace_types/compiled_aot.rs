//! Building a compiled trace from the metadata an AOT binary carries.

use super::*;

impl CompiledTrace {
    /// AOT install constructor: `plans` holds the tags the meta blob
    /// carries (typed-register side exits included, so GetUpval-heavy
    /// traces install correctly).
    ///
    /// Builds a [`CompiledTrace`] from the fields the AOT meta blob
    /// carries plus a deploy-resolved trace fn pointer.
    /// `body_writes` defaults to empty; `exit_hit_counts` has one count
    /// per exit of `plans`. The deploy `Vm` never records side traces against
    /// an AOT-installed parent (the recorder is invoked from the
    /// dispatch path; AOT traces install before any record can fire),
    /// so the side-trace bookkeeping defaults are sound.
    ///
    /// Traces whose runtime shape requires non-empty
    /// `per_exit_inline` (depth>0 inlined cmp side-exits with frame
    /// materialization chains) **must not** be emitted by the AOT
    /// pipeline — this constructor produces a CompiledTrace that the
    /// dispatcher would mis-restore for those traces. The AOT
    /// recorder driver (luna-aot side) filters to traces whose
    /// `per_exit_inline.is_empty()` before serializing.
    ///
    /// # Safety
    ///
    /// As [`CompiledTrace::from_parts`]: `entry` is code with the
    /// [`TraceFn`] ABI that lives as long as the program (an AOT trace
    /// linked into the binary's text segment), compiled against `plans`
    /// and the window size given (with `per_exit_inline` the inline exits
    /// of `plans`, in order), and holding the address of none of the
    /// trace's cells (they are made here).
    pub unsafe fn from_aot_meta(
        entry: TraceFn,
        head_pc: u32,
        n_ops: u32,
        dispatchable: bool,
        window_size: u32,
        plans: TracePlans,
        per_exit_inline: Vec<crate::jit::trace_types::InlineSideExit>,
    ) -> Self {
        // inline exits are those at depth > 0: the chains the
        // code holds come from `aot_inline_chain_resolver`, the copies here
        // from the wire format's `chain_bytes` (for shape decode and side
        // trace routing); no side compares the other's pointers
        let exit_hit_counts: TArc<[TCellU32]> =
            (0..plans.exit_count()).map(|_| TCellU32::new(0)).collect();
        let parts = CompiledTraceParts {
            head_pc,
            entry,
            n_ops,
            dispatchable,
            window_size,
            plans,
            per_exit_inline: TArc::from(per_exit_inline),
            exit_hit_counts,
            is_inline_abort_close: false,
            dispatch_off_reason: None,
            sinkable_sites_seen: 0,
            accum_bufferable_seen: 0,
            sunk_alloc_seen: 0,
            materialize_emit_count: 0,
            closure_seen: 0,
            inline_kinds: 0,
            inline_depth: 0,
            body_writes: Box::new([]),
            // AOT-install path never triggers a
            // down-recursion stitch (no recorder fires on the
            // deploy-side install). Always `None`.
            downrec_link: None,
            // AOT-install path doesn't emit a
            // DownRec close (no recorder fires on the deploy-side
            // install), so the candidate count is always `0`.
            downrec_multi_way_count: 0,
            tier_up: None,
            side_cell_exits: std::sync::Arc::new([]),
            side_cells: None,
            link_pc: None,
            loop_tags: None,
        };
        // SAFETY: `entry` is as the parts state and holds no address of
        // the cells made above (# Safety)
        unsafe { CompiledTrace::from_parts(parts) }
    }
}
