//! Building a compiled trace from the metadata an AOT binary carries.

use super::*;

impl CompiledTrace {
    /// AOT install constructor. Accepts `per_exit_tags`
    /// (typed-register side-exits) so GetUpval-heavy traces install
    /// correctly.
    ///
    /// Builds a [`CompiledTrace`] from the fields the AOT meta blob
    /// carries plus a deploy-resolved trace fn pointer.
    /// `per_exit_inline`, `tags_side_trace_ptrs`,
    /// `side_trace_cache`, `body_writes` default to empty / null;
    /// `exit_hit_counts` / `exit_side_trace_ptrs` are sized to
    /// `per_exit_tags.len() + 1` (each typed-exit slot + the global
    /// clean-tail). The deploy `Vm` never records side traces against
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
    /// linked into the binary's text segment), compiled against the tags
    /// and window size given, and holding the address of none of the
    /// trace's cells (they are made here).
    pub unsafe fn from_aot_meta(
        entry: TraceFn,
        head_pc: u32,
        n_ops: u32,
        dispatchable: bool,
        window_size: u32,
        entry_tags: TArc<[u8]>,
        exit_tags: TArc<[ExitTag]>,
        global_tag_res_kind: TagResKind,
        per_exit_tags: ExitTagTable,
        per_exit_inline: Vec<crate::jit::trace_types::InlineSideExit>,
    ) -> Self {
        // `inline_n` > 0 for inlined side exits at depth > 0: the chains the
        // code holds come from `aot_inline_chain_resolver`, the copies here
        // from the wire format's `chain_bytes` (for shape decode and side
        // trace routing); no side compares the other's pointers
        //
        // `exit_hit_counts` / `exit_side_trace_ptrs` sized to
        // `inline_n + tags_n + 1` (matches the dispatcher's
        // `hot_exit_iter` invariant).
        let inline_n = per_exit_inline.len();
        let total_slots = inline_n + per_exit_tags.len() + 1;
        let exit_hit_counts: TArc<[TCellU32]> = {
            let v: Vec<TCellU32> = (0..total_slots).map(|_| TCellU32::new(0)).collect();
            v.into()
        };
        let exit_side_trace_ptrs: TArc<[TCellPtr]> = {
            let v: Vec<TCellPtr> = (0..total_slots).map(|_| TCellPtr::null()).collect();
            v.into()
        };
        // Parallel `tags_side_trace_ptrs` slice — one cell per
        // per_exit_tags entry, all null. The AOT install never wires
        // a side trace (no recorder fires on this parent), so the
        // cells stay null for the binary's lifetime; sizing matches
        // the dispatcher's `per_exit_kinds.len() == tags_side_trace
        // _ptrs.len()` invariant the close handler asserts.
        let tags_side_trace_ptrs: Box<[TCellPtr]> =
            (0..per_exit_tags.len()).map(|_| TCellPtr::null()).collect();
        let parts = CompiledTraceParts {
            head_pc,
            entry,
            n_ops,
            dispatchable,
            window_size,
            exit_tags,
            global_tag_res_kind,
            entry_tags,
            per_exit_tags,
            per_exit_inline: TArc::from(per_exit_inline),
            exit_hit_counts,
            exit_side_trace_ptrs,
            tags_side_trace_ptrs,
            global_side_trace_ptr: Box::new(TCellPtr::null()),
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
        };
        // SAFETY: `entry` is as the parts state and holds no address of
        // the cells made above (# Safety); every side-trace cell is null
        unsafe { CompiledTrace::from_parts(parts) }
    }
}
