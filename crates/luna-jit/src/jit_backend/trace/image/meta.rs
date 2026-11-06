//! The dispatcher's data of a shared trace, made into another Vm's trace.

use super::*;

/// The data of a [`CompiledTrace`] beside its code, without the cells a
/// running trace updates.
pub(crate) struct Meta {
    head_pc: u32,
    n_ops: u32,
    dispatchable: bool,
    window_size: u32,
    /// Shared with every trace made from the image: an exit's plan is
    /// worked out once, by the first Vm to take the exit.
    pub(crate) plans: TracePlans,
    per_exit_inline: Box<[(u32, u32, Box<[FrameMaterializeInfo]>)]>,
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
            plans: ct.plans.clone(),
            per_exit_inline: ct
                .per_exit_inline
                .iter()
                .map(|e| {
                    (
                        e.cont_pc,
                        e.head_resume_pc,
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
        let per_exit_inline: Vec<InlineSideExit> = self
            .per_exit_inline
            .iter()
            .map(|(cont_pc, head_resume_pc, chain)| InlineSideExit {
                cont_pc: *cont_pc,
                head_resume_pc: *head_resume_pc,
                chain: TArc::from(&chain[..]),
            })
            .collect();
        let total = self.plans.exit_count();
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
            plans: self.plans.clone(),
            per_exit_inline: per_exit_inline.into(),
            exit_hit_counts: (0..total)
                .map(|_| TCellU32::new(0))
                .collect::<Vec<_>>()
                .into(),
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
            + self.plans.byte_size()
            + self
                .per_exit_inline
                .iter()
                .map(|(_, _, c)| 16 + 12 * c.len())
                .sum::<usize>()
            + 4 * self.body_writes.len()
    }
}
