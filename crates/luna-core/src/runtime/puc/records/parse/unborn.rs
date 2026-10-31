//! What a load makes before PUC does: luna compiles a chunk before it
//! counts it, so the objects the compiler makes are kept off the list of
//! objects until the count reaches the point where PUC makes each one,
//! which puts it on the list where PUC's is (the sweep goes over the list
//! in that order).

use super::*;
use crate::runtime::puc::count::LoadsAside;

/// What loads made before PUC does, off the list ([`Heap::set_aside`]).
pub(in crate::runtime::puc) struct Unborn {
    /// the objects, in the order they were made; null once forgotten
    objs: Vec<*mut GcHeader>,
    /// the first of `objs`: luna's names PUC makes when first used
    /// (`Heap::try_intern_name`)
    names: usize,
}

impl Unborn {
    /// Nothing set aside.
    pub(in crate::runtime::puc) fn new() -> Unborn {
        Unborn {
            objs: Vec::new(),
            names: 0,
        }
    }
}

impl Heap {
    /// [`Heap::set_aside`] while loads set objects aside.
    ///
    /// # Safety
    /// As [`Heap::link`]'s.
    #[cold]
    pub(in crate::runtime::puc) unsafe fn set_aside_now(&mut self, h: *mut GcHeader) -> bool {
        if self.puc.unborn.objs.try_reserve(1).is_err() {
            return false;
        }
        self.puc.unborn.objs.push(h);
        self.hook_reborn(h);
        // SAFETY: the caller's contract
        unsafe { self.detach_new(h) };
        true
    }

    /// PUC makes the object `h` now: on the list if it was set aside.
    ///
    /// # Safety
    /// `h` heads a live object of this heap.
    #[inline]
    pub(in crate::runtime::puc) unsafe fn born(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            if !Heap::is_detached(h) {
                return;
            }
            // one a marking of this cycle made black already (luna holds it
            // before PUC makes it) stays black: what holds it may be black
            // too (one gray is on a gray stack, traversed whatever its color)
            let f = (*h).flags();
            self.hook_born(h, f);
            self.link_born(h);
            if self.phase() == GcPhase::Propagate && is_black(f) {
                (*h).set_flags(f);
            }
        }
    }

    /// [`Heap::loads_aside_back`] after making luna's names: what is set
    /// aside so far is names, which a sweep may set aside again.
    pub(in crate::runtime::puc) fn names_aside_back(&mut self, a: LoadsAside) {
        self.loads_aside_back(a);
        self.puc.unborn.names = self.puc.unborn.objs.len();
    }

    /// The objects set aside at `from..to` of the list of them that are
    /// still aside go on the list, the others staying where they are.
    pub(in crate::runtime::puc) fn born_range(&mut self, (from, to): (usize, usize)) {
        let to = to.min(self.puc.unborn.objs.len());
        for i in from.min(to)..to {
            let h = self.puc.unborn.objs[i];
            if !h.is_null() {
                // SAFETY: an entry not forgotten is allocated; `born`
                // leaves one already on the list alone
                unsafe { self.born(h) };
            }
        }
    }

    /// The objects set aside since `base` (a length of the list of them)
    /// that PUC did not make go on the list: luna's own, or garbage of a
    /// load that failed.
    pub(in crate::runtime::puc) fn flush_unborn(&mut self, base: usize) {
        let mut rest = std::mem::take(&mut self.puc.unborn.objs);
        for &h in &rest[base.min(rest.len())..] {
            if !h.is_null() {
                // SAFETY: an entry not forgotten is allocated
                unsafe { self.born(h) };
            }
        }
        rest.truncate(base);
        self.puc.unborn.objs = rest;
    }

    /// How many objects are set aside, for [`Heap::flush_unborn`].
    pub(in crate::runtime::puc) fn unborn_len(&self) -> usize {
        self.puc.unborn.objs.len()
    }

    /// A sweep is over: what is set aside is white for the next cycle, as
    /// the sweep leaves every object it goes over (a marking of the cycle
    /// that ended may have reached it).
    pub(in crate::runtime::puc) fn whiten_unborn(&mut self) {
        let white = self.white();
        for h in self.unborn_objs() {
            // SAFETY: an object set aside is allocated; only its flags are
            // written
            unsafe { (*h).paint(white) };
        }
    }

    /// Whether `h`, a string PUC's sweep frees and luna keeps, is one of
    /// luna's names set aside at first: the sweep sets it aside again,
    /// for PUC's making it again to put it at the list's head.
    pub(in crate::runtime::puc) fn name_aside_again(&self, h: *mut GcHeader) -> bool {
        self.puc.unborn.objs[..self.puc.unborn.names].contains(&h)
    }

    /// A sweep starts: the entries of objects set aside that went on the
    /// list since are forgotten (made null), as the sweep may free them
    /// while a load goes on (its checkpoints run collection steps). luna's
    /// names, which it keeps and the sweep may set aside again, stay.
    pub(in crate::runtime::puc) fn forget_born_unborn(&mut self) {
        let from = self.puc.unborn.names;
        for h in self.puc.unborn.objs.iter_mut().skip(from) {
            // SAFETY: as in `unborn_objs`
            if !h.is_null() && !unsafe { Heap::is_detached(*h) } {
                *h = std::ptr::null_mut();
            }
        }
    }

    /// The objects still set aside: allocated, off the lists.
    pub(in crate::runtime::puc) fn unborn_objs(&self) -> impl Iterator<Item = *mut GcHeader> + '_ {
        // SAFETY: an entry not forgotten (made null) is allocated: an object
        // set aside is off the lists, which only a sweep frees from, and one
        // that went on the list since is forgotten as the next sweep
        // begins; only its link is read
        let aside = |h: &*mut GcHeader| !h.is_null() && unsafe { Heap::is_detached(*h) };
        self.puc.unborn.objs.iter().copied().filter(aside)
    }
}
