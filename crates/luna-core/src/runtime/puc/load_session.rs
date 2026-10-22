//! A load as the model sees it: luna compiles a chunk before it counts
//! it, so what the compiler builds is made uncounted and set aside, and
//! counted afterwards by the replay of PUC's parser. The three tokens here
//! pair the starts and ends of that: a load's span (what it set aside and
//! PUC never made goes on the list at its end), the building (counting off,
//! objects set aside), and the point a memory error unwinds the load to.
//!
//! The tokens hold no borrow of the heap (the VM runs the compiler between
//! start and end), so pairing is checked when one is dropped unused: a
//! debug or gc-verify build panics, unless the thread is unwinding already (a memory
//! error unwinds a load past its tokens, to [`LoadUnwind`]).

use super::*;
use crate::runtime::puc::loads::ParseMark;
use crate::runtime::puc::records::parse::LoadsAside;
use crate::runtime::puc::state::{CountingMark, CountingOff};

/// A load from its start to its end ([`Heap::load_begins`],
/// [`Heap::load_ends`]).
#[must_use = "a load's span ends with `Heap::load_ends`"]
pub(crate) struct LoadSpan {
    base: usize,
    ended: bool,
}

/// luna building a load's objects ahead of PUC ([`Heap::build_load`],
/// [`Heap::load_built`]).
#[must_use = "building ends with `Heap::load_built`"]
pub(crate) struct Building {
    off: Option<CountingOff>,
    aside: Option<LoadsAside>,
}

/// Where a load stood when it began, for a memory error to unwind it to
/// ([`Heap::load_unwind_point`], [`Heap::unwind_load`]).
#[must_use]
pub(crate) struct LoadUnwind {
    counting: CountingMark,
    frames: ParseMark,
    aside: LoadsAside,
    unborn: usize,
}

fn unpaired(what: &str) {
    if crate::runtime::gc_hooks::DEBUG_CHECKS && !std::thread::panicking() {
        panic!("{what} dropped without its end");
    }
}

impl Drop for LoadSpan {
    fn drop(&mut self) {
        if !self.ended {
            unpaired("a load's span");
        }
    }
}

impl Drop for Building {
    fn drop(&mut self) {
        if self.off.is_some() {
            unpaired("a load's building");
        }
    }
}

impl Heap {
    /// A load begins.
    #[inline]
    pub(crate) fn load_begins(&self) -> LoadSpan {
        LoadSpan {
            base: self.unborn_len(),
            ended: false,
        }
    }

    /// The load of `s` ends: what it set aside that PUC did not make (luna's
    /// own, or the garbage of a load that failed) goes on the list.
    #[inline]
    pub(crate) fn load_ends(&mut self, mut s: LoadSpan) {
        s.ended = true;
        self.flush_unborn(s.base);
    }

    /// luna builds a load's objects now, before PUC's parser makes them:
    /// nothing is counted, and with `aside` (a counted load) the objects
    /// are set aside until the replay reaches where PUC makes each.
    #[inline]
    pub(crate) fn build_load(&mut self, aside: bool) -> Building {
        let off = self.counting_off();
        Building {
            off: Some(off),
            aside: Some(self.set_loads_aside(aside)),
        }
    }

    /// The building `b` started is done: counting as it was.
    #[inline]
    pub(crate) fn load_built(&mut self, mut b: Building) {
        if let Some(a) = b.aside.take() {
            self.loads_aside_back(a);
        }
        if let Some(off) = b.off.take() {
            self.counting_on(off);
        }
    }

    /// Where loads stand now, for [`Heap::unwind_load`].
    #[inline]
    pub(crate) fn load_unwind_point(&self) -> LoadUnwind {
        LoadUnwind {
            counting: self.counting_mark(),
            frames: self.parse_mark(),
            aside: self.loads_aside_mark(),
            unborn: self.unborn_len(),
        }
    }

    /// A memory error unwound a load past its tokens: counting, the parser
    /// frames and the setting aside back to `u`, and what it set aside on
    /// the list.
    pub(crate) fn unwind_load(&mut self, u: LoadUnwind) {
        self.restore_counting(u.counting);
        self.restore_parse_frames(u.frames);
        self.loads_aside_back(u.aside);
        self.flush_unborn(u.unborn);
    }
}
