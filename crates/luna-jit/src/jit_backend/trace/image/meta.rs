//! The dispatcher's data of a shared trace, made into another Vm's trace.

use super::*;

/// The data of a [`CompiledTrace`] beside its code, without the cells a
/// running trace updates.
pub(crate) struct Meta {
    head_pc: u32,
    n_ops: u32,
    dispatchable: bool,
    window_size: u32,
    exit_tags: Box<[ExitTag]>,
    global_tag_res_kind: TagResKind,
    pub(crate) entry_tags: Box<[u8]>,
    per_exit_tags: ExitTagTable,
    per_exit_inline: Box<[(u32, u32, Box<[ExitTag]>, Box<[FrameMaterializeInfo]>)]>,
    is_inline_abort_close: bool,
    dispatch_off_reason: Option<&'static str>,
    sinkable_sites_seen: u32,
    accum_bufferable_seen: u32,
    sunk_alloc_seen: u32,
    materialize_emit_count: u32,
    closure_seen: u32,
    inline_kinds: u8,
    inline_depth: u8,
    body_writes: Box<[u32]>,
    downrec_link: Option<(u32, u32)>,
    downrec_multi_way_count: u8,
    pub(crate) tier_at: Option<u32>,
}

impl Meta {
    pub(crate) fn of(ct: &CompiledTraceParts) -> Meta {
        Meta {
            head_pc: ct.head_pc,
            n_ops: ct.n_ops,
            dispatchable: ct.dispatchable,
            window_size: ct.window_size,
            exit_tags: ct.exit_tags.iter().copied().collect(),
            global_tag_res_kind: ct.global_tag_res_kind,
            entry_tags: ct.entry_tags.iter().copied().collect(),
            per_exit_tags: ct.per_exit_tags.clone(),
            per_exit_inline: ct
                .per_exit_inline
                .iter()
                .map(|e| {
                    (
                        e.cont_pc,
                        e.head_resume_pc,
                        e.exit_tags.iter().copied().collect(),
                        e.chain.iter().copied().collect(),
                    )
                })
                .collect(),
            is_inline_abort_close: ct.is_inline_abort_close,
            dispatch_off_reason: ct.dispatch_off_reason,
            sinkable_sites_seen: ct.sinkable_sites_seen,
            accum_bufferable_seen: ct.accum_bufferable_seen,
            sunk_alloc_seen: ct.sunk_alloc_seen,
            materialize_emit_count: ct.materialize_emit_count,
            closure_seen: ct.closure_seen,
            inline_kinds: ct.inline_kinds,
            inline_depth: ct.inline_depth,
            body_writes: ct.body_writes.clone(),
            downrec_link: ct.downrec_link,
            downrec_multi_way_count: ct.downrec_multi_way_count,
            tier_at: ct.tier_up.as_ref().map(|t| t.at()),
        }
    }

    /// A fresh [`CompiledTrace`] (no code yet: `entry` is the placeholder),
    /// with its own cells. `tiered`: the trace runs the optimizing tier's
    /// code and moves no further.
    // kept out of line: inlined into the adoption path, its collects were
    // not, which costs every adopted trace more than the call
    #[inline(never)]
    pub(crate) fn instantiate(&self, tiered: bool, calls_at: u32) -> CompiledTraceParts {
        let arc = |t: &[ExitTag]| -> TArc<[ExitTag]> { TArc::from(t) };
        let per_exit_inline: Vec<InlineSideExit> = self
            .per_exit_inline
            .iter()
            .map(|(cont_pc, head_resume_pc, tags, chain)| InlineSideExit {
                cont_pc: *cont_pc,
                head_resume_pc: *head_resume_pc,
                exit_tags: arc(tags),
                chain: TArc::from(&chain[..]),
                side_trace_ptr: Box::new(TCellPtr::null()),
            })
            .collect();
        let total = per_exit_inline.len() + self.per_exit_tags.len() + 1;
        let tier_up = self
            .tier_at
            .filter(|_| !tiered)
            .map(|at| Box::new(TierUp::new(Box::new(TCellU32::new(0)), at, calls_at)));
        CompiledTraceParts {
            head_pc: self.head_pc,
            entry: placeholder_trace_fn,
            n_ops: self.n_ops,
            dispatchable: self.dispatchable,
            window_size: self.window_size,
            exit_tags: arc(&self.exit_tags),
            global_tag_res_kind: self.global_tag_res_kind,
            entry_tags: TArc::from(&self.entry_tags[..]),
            tags_side_trace_ptrs: (0..self.per_exit_tags.len())
                .map(|_| TCellPtr::null())
                .collect(),
            per_exit_tags: self.per_exit_tags.clone(),
            per_exit_inline: per_exit_inline.into(),
            exit_hit_counts: (0..total)
                .map(|_| TCellU32::new(0))
                .collect::<Vec<_>>()
                .into(),
            exit_side_trace_ptrs: (0..total)
                .map(|_| TCellPtr::null())
                .collect::<Vec<_>>()
                .into(),
            global_side_trace_ptr: Box::new(TCellPtr::null()),
            is_inline_abort_close: self.is_inline_abort_close,
            dispatch_off_reason: self.dispatch_off_reason,
            sinkable_sites_seen: self.sinkable_sites_seen,
            accum_bufferable_seen: self.accum_bufferable_seen,
            sunk_alloc_seen: self.sunk_alloc_seen,
            materialize_emit_count: self.materialize_emit_count,
            closure_seen: self.closure_seen,
            inline_kinds: self.inline_kinds,
            inline_depth: self.inline_depth,
            body_writes: self.body_writes.clone(),
            downrec_link: self.downrec_link,
            downrec_multi_way_count: self.downrec_multi_way_count,
            tier_up,
        }
    }

    pub(super) fn size(&self) -> usize {
        std::mem::size_of::<Meta>()
            + self.exit_tags.len()
            + self.entry_tags.len()
            + self
                .per_exit_tags
                .iter()
                .map(|(_, t)| 8 + t.len())
                .sum::<usize>()
            + self
                .per_exit_inline
                .iter()
                .map(|(_, _, t, c)| 16 + t.len() + 12 * c.len())
                .sum::<usize>()
            + 4 * self.body_writes.len()
    }
}
