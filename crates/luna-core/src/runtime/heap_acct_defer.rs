//! Counting set aside: what a load's reader function counts while the
//! parse runs is counted again where PUC's parser, which the load replays
//! afterwards, calls the reader.

use super::*;

/// What was counted while counting was set aside.
pub(crate) struct Deferred {
    bytes: isize,
    #[cfg(feature = "gc-trace")]
    lines: Vec<String>,
}

impl Heap {
    /// Set what is counted from now on aside; [`Heap::defer_end`] with the
    /// returned mark takes it back out.
    pub(crate) fn defer_begin(&mut self) -> usize {
        #[cfg(feature = "gc-trace")]
        self.trace.hold();
        self.bytes
    }

    /// What was counted since `mark`, taken out of the count.
    pub(crate) fn defer_end(&mut self, mark: usize) -> Deferred {
        let bytes = self.bytes as isize - mark as isize;
        self.bytes = mark;
        Deferred {
            bytes,
            #[cfg(feature = "gc-trace")]
            lines: self.trace.release(),
        }
    }

    /// Count what was set aside, now.
    pub(crate) fn defer_apply(&mut self, d: Deferred) {
        self.bytes = (self.bytes as isize + d.bytes) as usize;
        #[cfg(feature = "gc-trace")]
        self.trace.put(d.lines);
    }
}
