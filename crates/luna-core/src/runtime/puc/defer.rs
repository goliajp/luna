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
    /// A load calls its reader: what is counted from now on is set aside,
    /// with the objects made meanwhile, until [`Heap::reader_call_ends`]
    /// with the returned mark.
    pub(in crate::runtime) fn reader_call_begins(&mut self) -> DeferMark {
        self.hook_hold_lines();
        DeferMark {
            bytes: self.puc.count.bytes,
            objs: self.unborn_len(),
            aside: self.set_loads_aside(true),
        }
    }

    /// The reader returned: what it counted since `mark` is taken out of
    /// the count, the objects it made kept aside.
    pub(in crate::runtime) fn reader_call_ends(&mut self, mark: DeferMark) -> Deferred {
        self.loads_aside_back(mark.aside);
        let bytes = self.puc.count.bytes as isize - mark.bytes as isize;
        self.puc.count.bytes = mark.bytes;
        self.puc.loads.set_aside += bytes;
        Deferred {
            bytes,
            objs: (mark.objs, self.unborn_len()),
            lines: self.hook_release_lines(),
        }
    }

    /// The replay reaches where PUC's parser called the reader: what the
    /// call counted is counted now, its objects listed.
    pub(in crate::runtime) fn replay_reader_call(&mut self, d: Deferred) {
        self.puc.count.bytes = (self.puc.count.bytes as isize + d.bytes) as usize;
        self.puc.loads.set_aside -= d.bytes;
        self.born_range(d.objs);
        self.hook_put_lines(d.lines);
    }

    /// A load with a reader begins: where the counts set aside and the
    /// objects stand now, for [`Heap::reader_load_ends`].
    pub(in crate::runtime) fn reader_load_begins(&self) -> (isize, usize) {
        (self.puc.loads.set_aside, self.unborn_len())
    }

    /// A load that began at `mark` ends: what its reader's calls counted
    /// and no replay counted again is counted now, and the objects they
    /// made go on the list.
    pub(in crate::runtime) fn reader_load_ends(&mut self, (bytes, objs): (isize, usize)) {
        let left = self.puc.loads.set_aside - bytes;
        self.puc.count.bytes = (self.puc.count.bytes as isize + left) as usize;
        self.puc.loads.set_aside = bytes;
        self.flush_unborn(objs);
    }
}
