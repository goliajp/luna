//! A trace's register tags and the plans the dispatcher works out from them,
//! held once for every Vm that runs the trace's code.

use super::super::{ExitTag, ExitTagTable, TagResKind, classify_exit_tags};
use super::entry_plan::EntryPlan;
use super::exit_plan::ExitPlan;
use std::sync::Arc;

/// The register tags a trace's code was compiled against, and what the
/// dispatcher copies in on entry and writes back on each exit by them.
/// One trace's code shares one: a Vm that takes up a trace another Vm
/// compiled hands [`CompiledTraceParts`](super::super::CompiledTraceParts)
/// the plans of that compile ([`CompiledTrace::plans`](super::CompiledTrace::plans)),
/// so an exit's plan is worked out once, by the first Vm to take it.
#[derive(Clone)]
pub struct TracePlans(pub(crate) Arc<Plans>);

pub(crate) struct Plans {
    pub(crate) entry_tags: Box<[u8]>,
    pub(crate) exit_tags: Box<[ExitTag]>,
    pub(crate) global_tag_res_kind: TagResKind,
    pub(crate) per_exit_tags: ExitTagTable,
    pub(crate) entry_plan: EntryPlan,
    pub(crate) exit_plan: ExitPlan,
}

impl TracePlans {
    /// The plans of a trace compiled against `entry_tags` (as
    /// [`CompiledTrace::entry_tags`](super::CompiledTrace::entry_tags)),
    /// `exit_tags` (the clean tail's) and `per_exit_tags` (its typed side
    /// exits), with `inline_exits` inline side exits besides.
    pub fn new(
        entry_tags: &[u8],
        exit_tags: &[ExitTag],
        per_exit_tags: ExitTagTable,
        inline_exits: usize,
    ) -> Self {
        let n_exits = inline_exits + per_exit_tags.len() + 1;
        TracePlans(Arc::new(Plans {
            entry_plan: EntryPlan::of(entry_tags),
            exit_plan: ExitPlan::new(n_exits),
            entry_tags: entry_tags.into(),
            global_tag_res_kind: classify_exit_tags(exit_tags),
            exit_tags: exit_tags.into(),
            per_exit_tags,
        }))
    }

    /// The entry tags, as [`CompiledTrace::entry_tags`](super::CompiledTrace::entry_tags).
    pub fn entry_tags(&self) -> &[u8] {
        &self.0.entry_tags
    }

    /// The number of exits the plans cover: the inline side exits, plus
    /// the typed exits of the tag table, plus one for the clean tail. A
    /// trace's `exit_hit_counts` has this length.
    pub fn exit_count(&self) -> usize {
        self.0.exit_plan.len()
    }

    /// Bytes the tags, the entry plan and the per-exit cells take, roughly.
    /// Exit plans worked out as exits are first taken are not counted, so
    /// the value stays the same for as long as the plans live.
    pub fn byte_size(&self) -> usize {
        let p = &*self.0;
        std::mem::size_of::<Plans>()
            + p.entry_tags.len()
            + p.exit_tags.len()
            + p.per_exit_tags
                .iter()
                .map(|(_, t)| 8 + t.len())
                .sum::<usize>()
            + p.entry_plan.size()
            + p.exit_plan.cells_size()
    }
}

impl std::fmt::Debug for TracePlans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TracePlans")
            .field("entry_tags", &self.0.entry_tags)
            .field("exit_tags", &self.0.exit_tags)
            .field("exits", &self.exit_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_stays_the_same_once_an_exit_is_planned() {
        use ExitTag::*;
        let table = ExitTagTable::from_exits([(7, &[Int, Untouched][..])]);
        let p = TracePlans::new(&[3, 4], &[Int, Float], table, 1);
        assert_eq!(p.exit_count(), 3);
        let before = p.byte_size();
        let e =
            p.0.exit_plan
                .exit(1, p.0.per_exit_tags.tags(0))
                .expect("exit 1");
        assert_eq!(e.written.len(), 1);
        assert_eq!(p.byte_size(), before);
    }

    #[test]
    fn threads_taking_an_exit_first_together_share_one_plan() {
        use ExitTag::*;
        const N: usize = 8;
        let p = TracePlans::new(&[3], &[Int, Untouched, Float], ExitTagTable::default(), 0);
        let go = std::sync::Barrier::new(N);
        let got: Vec<usize> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..N)
                .map(|_| {
                    s.spawn(|| {
                        go.wait();
                        let e = p.0.exit_plan.exit(0, &p.0.exit_tags).expect("exit 0");
                        e as *const _ as usize
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().expect("thread")).collect()
        });
        assert!(got.iter().all(|&a| a == got[0]));
        let e = p.0.exit_plan.exit(0, &[]).expect("exit 0");
        assert_eq!((e.written.len(), e.untouched_end), (2, 2));
    }

    #[test]
    fn plans_cross_threads() {
        fn shared<T: Send + Sync>() {}
        shared::<TracePlans>();
    }
}
