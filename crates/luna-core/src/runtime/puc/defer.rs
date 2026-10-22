//! Counting set aside: what a load's reader function counts while the
//! parse runs is counted again where PUC's parser, which the load replays
//! afterwards, calls the reader. The objects the reader makes are set
//! aside with it, off the list, so no sweep frees one whose count is not
//! there yet; they go on the list where their count comes back. What no
//! replay counts again is counted back, its objects listed, when the load
//! ends.

use super::*;

/// Where a reader's call began: the count, the list of objects set
/// aside, and whether they were being set aside already.
pub(crate) struct DeferMark {
    bytes: usize,
    objs: usize,
    aside: crate::runtime::puc::records::parse::LoadsAside,
}

/// What was counted while counting was set aside.
pub(crate) struct Deferred {
    bytes: isize,
    /// where the objects made meanwhile are in the list of those set aside
    objs: (usize, usize),
    /// gc-trace: the lines held back meanwhile
    lines: crate::runtime::gc_hooks::HeldLines,
}

impl Heap {
    /// Set what is counted from now on aside, and the objects made with
    /// it; [`Heap::defer_end`] with the returned mark takes it back out.
    pub(crate) fn defer_begin(&mut self) -> DeferMark {
        self.hook_hold_lines();
        DeferMark {
            bytes: self.puc.bytes,
            objs: self.unborn_len(),
            aside: self.set_loads_aside(true),
        }
    }

    /// What was counted since `mark`, taken out of the count, and the
    /// objects made since, kept aside.
    pub(crate) fn defer_end(&mut self, mark: DeferMark) -> Deferred {
        self.loads_aside_back(mark.aside);
        let bytes = self.puc.bytes as isize - mark.bytes as isize;
        self.puc.bytes = mark.bytes;
        self.puc.set_aside += bytes;
        Deferred {
            bytes,
            objs: (mark.objs, self.unborn_len()),
            lines: self.hook_release_lines(),
        }
    }

    /// Count what was set aside, now.
    pub(crate) fn defer_apply(&mut self, d: Deferred) {
        self.puc.bytes = (self.puc.bytes as isize + d.bytes) as usize;
        self.puc.set_aside -= d.bytes;
        self.born_range(d.objs);
        self.hook_put_lines(d.lines);
    }

    /// Where the loads' set aside count and objects stand now, for
    /// [`Heap::defer_settle`].
    pub(crate) fn defer_mark(&self) -> (isize, usize) {
        (self.puc.set_aside, self.unborn_len())
    }

    /// A load that began at `mark` ends: what its reader's calls counted
    /// and no replay counted again is counted now, and the objects they
    /// made go on the list.
    pub(crate) fn defer_settle(&mut self, (bytes, objs): (isize, usize)) {
        let left = self.puc.set_aside - bytes;
        self.puc.bytes = (self.puc.bytes as isize + left) as usize;
        self.puc.set_aside = bytes;
        self.flush_unborn(objs);
    }
}
