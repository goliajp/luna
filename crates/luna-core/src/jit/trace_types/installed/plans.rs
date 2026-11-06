//! A trace's register tags and the plans the dispatcher works out from them,
//! held once for every Vm that runs the trace's code.

use super::super::{DecodedExit, ExitTag, ExitTagTable, TagResKind, classify_exit_tags};
use super::entry_plan::EntryPlan;
use super::exit_plan::{ExitPlan, PlannedExit};
use std::sync::Arc;

/// Where a looping trace's code counts its passes round the back edge,
/// as the backend that compiled it gave out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PassCount {
    /// In the count cell of the trace's tier-up, which every call of the
    /// trace shares, or nowhere.
    #[default]
    Shared,
    /// In this call's words past the window (`__private::trace_count_slot`
    /// and `trace_passed_slot`), so that an exit tells a call past its
    /// first back edge from one before it. Given with the registers the
    /// loop writes without reading them, each with the kind the back edge
    /// leaves it in: an exit a call takes past its first back edge, whose
    /// tag for such a register is [`ExitTag::Untouched`], restores it with
    /// that kind (before the first back edge it holds what it held on
    /// entry). Each kind is one [`kept_past_back_edge`] allows.
    PerCall(Box<[(u16, ExitTag)]>),
}

/// Whether a register a looping trace writes without reading can be given
/// kind `k` past the first back edge ([`PassCount::PerCall`]): a value the
/// collector does not trace, so a write-back of one is never a stale object.
pub fn kept_past_back_edge(k: ExitTag) -> bool {
    matches!(
        k,
        ExitTag::Int | ExitTag::Float | ExitTag::Bool | ExitTag::Nil
    )
}

/// The register tags a trace's code was compiled against, and what the
/// dispatcher copies in on entry and writes back on each exit by them.
/// One trace's code shares one: a Vm that takes up a trace another Vm
/// compiled hands [`CompiledTraceParts`](super::super::CompiledTraceParts)
/// a clone of the plans of that compile, so an exit's plan is worked out
/// once, by the first Vm to take it.
///
/// The exits are numbered as a trace's hit counts are: the inline side
/// exits, then the typed exits, then the clean tail.
#[derive(Clone)]
pub struct TracePlans(pub(crate) Arc<Plans>);

pub(crate) struct Plans {
    pub(crate) entry_tags: Box<[u8]>,
    pub(crate) exit_tags: Box<[ExitTag]>,
    pub(crate) global_tag_res_kind: TagResKind,
    pub(crate) inline_tags: ExitTagTable,
    pub(crate) per_exit_tags: ExitTagTable,
    pub(crate) pass_count: PassCount,
    pub(crate) entry_plan: EntryPlan,
    exit_plan: ExitPlan,
}

impl TracePlans {
    /// The plans of a trace compiled against `entry_tags` (as
    /// [`CompiledTrace::entry_tags`](super::CompiledTrace::entry_tags)),
    /// `exit_tags` (the clean tail's), `per_exit_tags` (its typed side
    /// exits) and `inline_tags` (its inline side exits: each one's resume
    /// pc and tags over the whole window, in site order), whose code counts
    /// its passes as `pass_count` says.
    ///
    /// # Panics
    /// If [`PassCount::PerCall`] gives a register twice, or a kind
    /// [`kept_past_back_edge`] does not allow.
    pub fn new(
        entry_tags: &[u8],
        exit_tags: &[ExitTag],
        per_exit_tags: ExitTagTable,
        inline_tags: ExitTagTable,
        pass_count: PassCount,
    ) -> Self {
        let n_exits = inline_tags.len() + per_exit_tags.len() + 1;
        TracePlans(Arc::new(Plans {
            entry_plan: EntryPlan::of(entry_tags),
            exit_plan: ExitPlan::new(n_exits),
            entry_tags: entry_tags.into(),
            global_tag_res_kind: classify_exit_tags(exit_tags),
            exit_tags: exit_tags.into(),
            inline_tags,
            per_exit_tags,
            pass_count: sorted(pass_count),
        }))
    }

    /// The entry tags, as [`CompiledTrace::entry_tags`](super::CompiledTrace::entry_tags).
    pub fn entry_tags(&self) -> &[u8] {
        &self.0.entry_tags
    }

    /// The number of exits the plans cover: the inline side exits, plus
    /// the typed exits, plus one for the clean tail. A trace's
    /// `exit_hit_counts` has this length.
    pub fn exit_count(&self) -> usize {
        self.0.exit_count()
    }

    /// Bytes the tags, the entry plan and the per-exit cells take, roughly.
    /// Exit plans worked out as exits are first taken are not counted, so
    /// the value stays the same for as long as the plans live.
    pub fn byte_size(&self) -> usize {
        let p = &*self.0;
        let table = |t: &ExitTagTable| t.iter().map(|(_, t)| 8 + t.len()).sum::<usize>();
        std::mem::size_of::<Plans>()
            + p.entry_tags.len()
            + p.exit_tags.len()
            + table(&p.inline_tags)
            + table(&p.per_exit_tags)
            + std::mem::size_of_val(p.back_edge_kinds())
            + p.entry_plan.size()
            + p.exit_plan.cells_size()
    }
}

/// `c` with its registers in order, checked.
fn sorted(c: PassCount) -> PassCount {
    let PassCount::PerCall(kinds) = c else {
        return c;
    };
    let mut v = kinds.into_vec();
    for &(r, k) in &v {
        assert!(
            kept_past_back_edge(k),
            "register {r}: back edge kind {k:?} is not a kind the collector skips"
        );
    }
    v.sort_unstable_by_key(|e| e.0);
    assert!(
        v.windows(2).all(|w| w[0].0 != w[1].0),
        "a register given two back edge kinds"
    );
    PassCount::PerCall(v.into())
}

impl Plans {
    /// The exits numbered.
    pub(crate) fn exit_count(&self) -> usize {
        self.exit_plan.len()
    }

    /// Exit `i`'s tags: an inline exit's over the whole window, a typed
    /// exit's or the clean tail's over the head frame (the clean tail's
    /// for any `i` past the typed exits).
    pub(crate) fn exit_tags(&self, i: usize) -> &[ExitTag] {
        let inline_n = self.inline_tags.len();
        if i < inline_n {
            self.inline_tags.tags(i)
        } else if i < inline_n + self.per_exit_tags.len() {
            self.per_exit_tags.tags(i - inline_n)
        } else {
            &self.exit_tags
        }
    }

    /// The pc exit `i` resumes at, `None` for the clean tail (which goes on
    /// at the trace's head).
    pub(crate) fn exit_cont_pc(&self, i: usize) -> Option<u32> {
        let inline_n = self.inline_tags.len();
        if i < inline_n {
            Some(self.inline_tags.cont_pc(i))
        } else {
            self.per_exit_tags.get(i - inline_n).map(|(pc, _)| pc)
        }
    }

    /// Exit `i`'s plan, worked out from its tags the first time.
    #[inline]
    pub(crate) fn exit(&self, i: usize) -> Option<&PlannedExit> {
        let cell = self.exit_plan.cell(i)?;
        Some(cell.get_or_init(|| Box::new(PlannedExit::of(self.exit_tags(i)))))
    }

    /// The exit a trace's return `raw_ret` names (see
    /// [`super::super::decode_exit_shape`]).
    #[inline(always)]
    pub(crate) fn decode(&self, raw_ret: u64) -> DecodedExit<'_> {
        super::super::decode_exit_shape(
            raw_ret,
            &self.inline_tags,
            &self.per_exit_tags,
            &self.exit_tags,
        )
    }

    /// The registers [`PassCount::PerCall`] gives a kind past the first
    /// back edge, by register; none for code that counts otherwise.
    pub(crate) fn back_edge_kinds(&self) -> &[(u16, ExitTag)] {
        match &self.pass_count {
            PassCount::PerCall(k) => k,
            PassCount::Shared => &[],
        }
    }
}

impl std::fmt::Debug for TracePlans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TracePlans")
            .field("entry_tags", &self.0.entry_tags)
            .field("exit_tags", &self.0.exit_tags)
            .field("exits", &self.exit_count())
            .field("pass_count", &self.0.pass_count)
            .finish()
    }
}

#[cfg(test)]
mod tests;
