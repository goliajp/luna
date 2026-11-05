//! What a dispatch checks and copies into a trace's register buffer before
//! it runs, worked out once from the trace's entry tags.

use super::super::ENTRY_TAG_ANY;

/// A register a trace checks on entry: its slot in the head frame and the
/// tag the trace was compiled for. Two of these fit in a word, laid out so
/// that generated code can read the plan as well.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EntrySlot {
    pub(crate) slot: u16,
    pub(crate) tag: u8,
}

/// The head frame's registers a trace takes on entry, from its entry tags
/// (`CompiledTrace::entry_tags`): the ones it checks, and the ones it takes
/// unchecked (entry tag `ENTRY_TAG_ANY`), which only a side trace run from
/// one of its exits reads from the buffers.
#[derive(Debug, Default)]
pub(crate) struct EntryPlan {
    pub(crate) checked: Box<[EntrySlot]>,
    pub(crate) unchecked: Box<[u16]>,
    /// The registers the entry tags cover; a frame's registers past them
    /// are checked against their own tags.
    pub(crate) len: usize,
}

impl EntryPlan {
    pub(crate) fn of(entry_tags: &[u8]) -> Self {
        let slots = || (0u16..).zip(entry_tags.iter().copied());
        EntryPlan {
            checked: slots()
                .filter(|&(_, tag)| tag != ENTRY_TAG_ANY)
                .map(|(slot, tag)| EntrySlot { slot, tag })
                .collect(),
            unchecked: slots()
                .filter(|&(_, tag)| tag == ENTRY_TAG_ANY)
                .map(|(slot, _)| slot)
                .collect(),
            len: entry_tags.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_splits_the_entry_tags_by_slot() {
        let p = EntryPlan::of(&[3, ENTRY_TAG_ANY, 5, ENTRY_TAG_ANY]);
        assert_eq!(
            &*p.checked,
            &[EntrySlot { slot: 0, tag: 3 }, EntrySlot { slot: 2, tag: 5 }]
        );
        assert_eq!(&*p.unchecked, &[1, 3]);
        assert_eq!(p.len, 4);
    }
}
