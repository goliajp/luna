//! A root trace's entry side traces: traces recorded from its head for
//! entries its checks refuse, which the dispatcher tries once it refuses
//! one. Never removed from the list, so that a place in it names the same
//! trace for as long as the root lives (see `ParentId`).

use super::super::*;

/// Entry side traces a root trace holds at most.
pub(crate) const ENTRY_SIDE_CAP: usize = 3;

/// What recording entry side traces at a root may fail, in the units of a
/// head's budget of failures (`vm::exec::trace::cache`).
pub(crate) const ENTRY_SIDE_BUDGET: u32 = 48;

/// What a trace holds as a root of entry side traces, or as one.
pub(crate) struct EntrySides {
    /// The entry side traces, in the order the dispatcher tries them.
    pub(crate) list: TRefLock<Vec<TArc<CompiledTrace>>>,
    /// What recording more may still fail; none left: none is recorded.
    pub(crate) budget: TCellU32,
    /// For a root at pc 0: calls its checks and every entry side trace's
    /// refused since recording one was last considered.
    pub(crate) call_refusals: TCellU32,
    /// This trace is an entry side trace, set before it is attached.
    pub(crate) is_one: TCellBool,
    /// The place in `list` of the one that took the last entry the root
    /// refused, tried first at the next the root refuses; [`NO_LAST`] when
    /// none took it. The root's own entries leave it as it is: the dispatch
    /// pays nothing for it before the root refuses.
    pub(crate) last: TCellU32,
    /// The times the one tried first did not take the entry: past
    /// [`MAX_MISSES`] none is tried first any more (the types change at
    /// most entries).
    pub(crate) misses: TCellU32,
    /// The word the root refused entries by at the last time its loop head
    /// was hot, and how many times round the loop it has refused entries by
    /// that word since: an entry side trace is recorded only for one it
    /// refuses for [`ENTRY_SIDE_HEAT`] times round.
    pub(crate) heat_word: TCellU32,
    pub(crate) heat: TCellU32,
}

/// The times round a loop its trace refuses entries of one kind before an
/// entry side trace is recorded for them: as many as make a loop hot by
/// default, however low the threshold is set.
pub(crate) const ENTRY_SIDE_HEAT: u32 = crate::jit::trace::TRACE_HOT_THRESHOLD;

/// [`EntrySides::misses`] past which no entry side trace is tried first.
pub(crate) const MAX_MISSES: u32 = 16;

/// [`EntrySides::last`] when no entry side trace took the last entry.
pub(crate) const NO_LAST: u32 = u32::MAX;

impl Default for EntrySides {
    fn default() -> Self {
        EntrySides {
            list: TRefLock::new(Vec::new()),
            budget: TCellU32::new(ENTRY_SIDE_BUDGET),
            call_refusals: TCellU32::new(0),
            is_one: TCellBool::new(false),
            last: TCellU32::new(NO_LAST),
            misses: TCellU32::new(0),
            heat_word: TCellU32::new(crate::runtime::function::TRACE_HEAD_NOT_REFUSED),
            heat: TCellU32::new(0),
        }
    }
}
