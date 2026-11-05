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

/// The head frame's registers a trace checks on entry, from its entry tags
/// (`CompiledTrace::entry_tags`): the ones whose tag is not
/// `ENTRY_TAG_ANY`, in slot order. Most traces check a handful, which sit
/// in the trace itself; more than [`ENTRY_PLAN_INLINE`] take an
/// allocation. The unchecked registers, which only a side trace run from
/// one of its exits reads from the buffers, are read off the entry tags.
#[derive(Clone, Debug)]
pub(crate) struct EntryPlan {
    checked: Checked,
    /// The registers the entry tags cover; a frame's registers past them
    /// are checked against their own tags.
    len: u32,
}

/// Checked registers an [`EntryPlan`] holds without an allocation.
pub(crate) const ENTRY_PLAN_INLINE: usize = 12;

// a trace copies its plan in when installed: the rare long one is shared
#[derive(Clone, Debug)]
enum Checked {
    Inline(u8, [EntrySlot; ENTRY_PLAN_INLINE]),
    Heap(std::sync::Arc<[EntrySlot]>),
}

impl Default for EntryPlan {
    fn default() -> Self {
        EntryPlan::of(&[])
    }
}

impl EntryPlan {
    pub(crate) fn of(entry_tags: &[u8]) -> Self {
        let checked = || {
            (0u16..)
                .zip(entry_tags)
                .filter(|&(_, &tag)| tag != ENTRY_TAG_ANY)
                .map(|(slot, &tag)| EntrySlot { slot, tag })
        };
        let n = entry_tags.iter().filter(|&&t| t != ENTRY_TAG_ANY).count();
        let checked = if n <= ENTRY_PLAN_INLINE {
            let mut a = [EntrySlot { slot: 0, tag: 0 }; ENTRY_PLAN_INLINE];
            for (d, e) in a.iter_mut().zip(checked()) {
                *d = e;
            }
            Checked::Inline(n as u8, a)
        } else {
            Checked::Heap(checked().collect())
        };
        EntryPlan {
            checked,
            len: entry_tags.len() as u32,
        }
    }

    /// The registers checked on entry, in slot order.
    #[inline]
    pub(crate) fn checked(&self) -> &[EntrySlot] {
        match &self.checked {
            Checked::Inline(n, a) => &a[..*n as usize],
            Checked::Heap(b) => b,
        }
    }

    /// Bytes the plan takes beyond itself.
    pub(crate) fn size(&self) -> usize {
        match &self.checked {
            Checked::Inline(..) => 0,
            Checked::Heap(b) => 4 * b.len(),
        }
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_lists_the_checked_registers() {
        let p = EntryPlan::of(&[3, ENTRY_TAG_ANY, 5, ENTRY_TAG_ANY]);
        assert_eq!(
            p.checked(),
            &[EntrySlot { slot: 0, tag: 3 }, EntrySlot { slot: 2, tag: 5 }]
        );
        assert_eq!(p.len(), 4);
        // more than fit in the trace: allocated, the same list
        let tags: Vec<u8> = (0..40)
            .map(|i| if i % 3 == 0 { ENTRY_TAG_ANY } else { 2 })
            .collect();
        let p = EntryPlan::of(&tags);
        assert!(matches!(p.checked, Checked::Heap(_)));
        let want: Vec<u16> = (0..40).filter(|i| i % 3 != 0).collect();
        assert_eq!(p.checked().iter().map(|e| e.slot).collect::<Vec<_>>(), want);
    }
}
