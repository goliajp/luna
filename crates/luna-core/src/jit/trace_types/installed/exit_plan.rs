//! What each exit of a trace writes back, worked out once from its exit
//! tags: the registers it wrote, with their kinds.

use super::super::{ExitTag, ExitTagTable, InlineSideExit};

/// A register an exit wrote: its slot in the trace's window and its kind.
/// Laid out so that generated code can read the plan as well.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExitSlot {
    pub(crate) slot: u16,
    pub(crate) kind: ExitTag,
}

/// Per exit, numbered as its hit counts are (the inline side exits, the
/// typed exits of the tag table, then the trace's global tags): the
/// registers it wrote, in slot order, and how far its tags reach.
#[derive(Debug, Default)]
pub(crate) struct ExitPlan {
    /// `(start, end, untouched_end, len)`: `slots[start..end]` were
    /// written; every register the exit left untouched lies below
    /// `untouched_end`; its tags cover `len` registers.
    exits: Box<[(u32, u32, u32, u32)]>,
    slots: Box<[ExitSlot]>,
}

/// One exit of an [`ExitPlan`].
pub(crate) struct PlannedExit<'a> {
    pub(crate) written: &'a [ExitSlot],
    pub(crate) untouched_end: usize,
    pub(crate) len: usize,
}

impl ExitPlan {
    pub(crate) fn of(inline: &[InlineSideExit], table: &ExitTagTable, global: &[ExitTag]) -> Self {
        let mut exits = Vec::new();
        let mut slots = Vec::new();
        let all = (inline.iter().map(|e| &*e.exit_tags))
            .chain((0..table.len()).map(|i| table.tags(i)))
            .chain(std::iter::once(global));
        for tags in all {
            let start = slots.len() as u32;
            let mut untouched_end = 0;
            for (slot, &kind) in (0u16..).zip(tags) {
                if kind == ExitTag::Untouched {
                    untouched_end = u32::from(slot) + 1;
                } else {
                    slots.push(ExitSlot { slot, kind });
                }
            }
            exits.push((start, slots.len() as u32, untouched_end, tags.len() as u32));
        }
        ExitPlan {
            exits: exits.into(),
            slots: slots.into(),
        }
    }

    /// Exit `i` (its hit-count index).
    #[inline]
    pub(crate) fn exit(&self, i: usize) -> Option<PlannedExit<'_>> {
        let &(start, end, untouched_end, len) = self.exits.get(i)?;
        Some(PlannedExit {
            written: self.slots.get(start as usize..end as usize)?,
            untouched_end: untouched_end as usize,
            len: len as usize,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_exit_lists_the_registers_it_wrote() {
        use ExitTag::*;
        let table = ExitTagTable::from_exits([(7, &[Int, Untouched, Float][..])]);
        let p = ExitPlan::of(&[], &table, &[Untouched, Nil]);
        let e = p.exit(0).expect("typed exit");
        assert_eq!(
            e.written,
            &[
                ExitSlot { slot: 0, kind: Int },
                ExitSlot {
                    slot: 2,
                    kind: Float
                }
            ]
        );
        assert_eq!((e.untouched_end, e.len), (2, 3));
        let g = p.exit(1).expect("global exit");
        assert_eq!(g.written, &[ExitSlot { slot: 1, kind: Nil }]);
        assert_eq!((g.untouched_end, g.len), (1, 2));
        assert!(p.exit(2).is_none());
    }
}
