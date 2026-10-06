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
    /// `entry` must be a valid `unsafe extern "C" fn(*mut i64) -> i64`
    /// living in the binary's text segment (linker-resolved from the
    /// AOT-emitted trace `.o`). The constructor doesn't validate this
    /// — `install_aot_trace` is the next gate.
    pub fn from_aot_meta(
        entry: TraceFn,
        head_pc: u32,
        n_ops: u32,
        dispatchable: bool,
        window_size: u32,
        entry_tags: TArc<[u8]>,
        exit_tags: TArc<[ExitTag]>,
        global_tag_res_kind: TagResKind,
        per_exit_tags: Vec<(u32, TArc<[ExitTag]>)>,
        per_exit_inline: Vec<crate::jit::trace_types::InlineSideExit>,
    ) -> Self {
        // `inline_n` non-zero when
        // the AOT trace ships depth>0 inlined cmp side-exits. The
        // chain pointers baked into the trace mcode are populated by
        // the deploy-side `aot_inline_chain_resolver`; the
        // `per_exit_inline` entries here own a separate Rc<[...]>
        // rebuilt from the v3 wire format's `chain_bytes`, used by
        // the dispatcher for side-exit shape decode + side-trace
        // routing. Neither side compares pointers — both consume the
        // metadata they own.
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
        // Parallel `tags_side_trace_ptrs` slice — one Box per
        // per_exit_tags entry, all null. The AOT install never wires
        // a side trace (no recorder fires on this parent), so the
        // cells stay null for the binary's lifetime; sizing matches
        // the dispatcher's `per_exit_kinds.len() == tags_side_trace
        // _ptrs.len()` invariant the close handler asserts.
        let tags_side_trace_ptrs: TArc<[Box<TCellPtr>]> = {
            let v: Vec<Box<TCellPtr>> = (0..per_exit_tags.len())
                .map(|_| Box::new(TCellPtr::null()))
                .collect();
            v.into()
        };
        CompiledTrace {
            head_pc,
            entry,
            n_ops,
            dispatchable,
            window_size,
            exit_tags,
            global_tag_res_kind,
            entry_tags,
            per_exit_tags: TArc::from(per_exit_tags),
            per_exit_inline: TArc::from(per_exit_inline),
            exit_hit_counts,
            exit_side_trace_ptrs,
            tags_side_trace_ptrs,
            global_side_trace_ptr: Box::new(TCellPtr::null()),
            side_trace_cache: TRefLock::new(std::collections::HashMap::new()),
            side_children: TRefLock::new(std::collections::HashMap::new()),
            has_any_side_wired: TCellBool::new(false),
            is_inline_abort_close: false,
            dispatch_off_reason: None,
            sinkable_sites_seen: 0,
            accum_bufferable_seen: 0,
            sunk_alloc_seen: 0,
            materialize_emit_count: 0,
            closure_seen: 0,
            inline_kinds: 0,
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
        }
    }
}
