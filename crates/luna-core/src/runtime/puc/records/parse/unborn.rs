//! What a load makes before PUC does: luna compiles a chunk before it
//! counts it, so the objects the compiler makes are kept off the list of
//! objects until the count reaches the point where PUC makes each one,
//! which puts it on the list where PUC's is (the sweep goes over the list
//! in that order).

use super::*;

/// In the `next` of an object set aside: PUC has not made it yet.
const UNBORN: *mut GcHeader = ptr::without_provenance_mut(1);

/// Whether `h` is set aside: PUC has not made it yet.
///
/// # Safety
/// `h` heads an allocated object.
#[inline]
unsafe fn is_unborn(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*h).next == UNBORN }
}

/// Whether loads were set aside before (see [`Heap::set_loads_aside`]).
#[must_use = "loads stay as set until the token goes back to `Heap::loads_aside_back`"]
pub(in crate::runtime) struct LoadsAside(bool);

impl Heap {
    /// A new object a load makes before PUC does (luna compiles first),
    /// kept off the list until PUC makes it, which puts it where PUC's is.
    /// False when there is no room to note it (it goes on the list now).
    ///
    /// # Safety
    /// As [`Heap::link`]'s.
    #[cold]
    pub(in crate::runtime) unsafe fn set_aside(&mut self, h: *mut GcHeader) -> bool {
        if self.puc.unborn.try_reserve(1).is_err() {
            return false;
        }
        self.puc.unborn.push(h);
        self.hook_reborn(h);
        // SAFETY: the caller's contract
        unsafe {
            (*h).next = UNBORN;
            // white as a new object, for the barriers that reach it first
            (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | self.white());
        }
        true
    }

    /// PUC makes the object `h` now: on the list if it was set aside.
    ///
    /// # Safety
    /// `h` heads a live object of this heap.
    #[inline]
    pub(in crate::runtime) unsafe fn born(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            if !is_unborn(h) {
                return;
            }
            // one a marking of this cycle made black already (luna holds it
            // before PUC makes it) stays black: what holds it may be black
            // too (one gray is on a gray stack, traversed whatever its color)
            let f = (*h).flags;
            self.hook_born(h, f);
            self.link_now(h);
            if self.phase() == GcPhase::Propagate && is_black(f) {
                (*h).flags = f;
            }
        }
    }

    /// Set what a load makes before PUC does aside (`on`), or no more,
    /// until the token goes back to [`Heap::loads_aside_back`].
    #[inline]
    pub(in crate::runtime) fn set_loads_aside(&mut self, on: bool) -> LoadsAside {
        LoadsAside(std::mem::replace(&mut self.puc.unborn_on, on))
    }

    /// Where loads were set aside at some point (a load unwound by running
    /// out of memory goes back there).
    #[inline]
    pub(in crate::runtime) fn loads_aside_mark(&self) -> LoadsAside {
        LoadsAside(self.puc.unborn_on)
    }

    /// Back to what [`Heap::set_loads_aside`] changed.
    #[inline]
    pub(in crate::runtime) fn loads_aside_back(&mut self, a: LoadsAside) {
        self.puc.unborn_on = a.0;
    }

    /// The objects set aside at `from..to` of the list of them that are
    /// still aside go on the list, the others staying where they are.
    pub(in crate::runtime) fn born_range(&mut self, (from, to): (usize, usize)) {
        let to = to.min(self.puc.unborn.len());
        for i in from.min(to)..to {
            let h = self.puc.unborn[i];
            // SAFETY: an object set aside is allocated; `born` leaves one
            // already on the list alone
            unsafe { self.born(h) };
        }
    }

    /// The objects set aside since `base` (a length of the list of them)
    /// that PUC did not make go on the list: luna's own, or garbage of a
    /// load that failed.
    pub(in crate::runtime) fn flush_unborn(&mut self, base: usize) {
        let mut rest = std::mem::take(&mut self.puc.unborn);
        for &h in &rest[base.min(rest.len())..] {
            // SAFETY: an object set aside is allocated, and only here
            unsafe { self.born(h) };
        }
        rest.truncate(base);
        self.puc.unborn = rest;
    }

    /// Move the object `h` on the list to just before `anchor`, where the
    /// sweep reaches it right before `anchor`: 5.3's upvalues are not on
    /// PUC's list but freed with the closure that holds them, before it,
    /// and a load's closure is older than the upvalues it gets. Both are
    /// near the head (made by the load that is finishing); `h` stays where
    /// it is when it is not on the list or the sweep stands at it.
    ///
    /// # Safety
    /// `h` and `anchor` head live objects of this heap.
    pub(crate) unsafe fn link_before(&mut self, h: *mut GcHeader, anchor: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            if is_unborn(h) || is_unborn(anchor) {
                return;
            }
            self.move_before(h, anchor);
        }
    }

    /// How many objects are set aside, for [`Heap::flush_unborn`].
    pub(in crate::runtime) fn unborn_len(&self) -> usize {
        self.puc.unborn.len()
    }

    /// A sweep is over: what is set aside is white for the next cycle, as
    /// the sweep leaves every object it goes over (a marking of the cycle
    /// that ended may have reached it).
    pub(in crate::runtime) fn whiten_unborn(&mut self) {
        let white = self.white();
        for h in self.unborn_objs() {
            // SAFETY: an object set aside is allocated; only its flags are
            // written
            unsafe { (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | white) };
        }
    }

    /// The objects still set aside: allocated, off the lists.
    pub(in crate::runtime) fn unborn_objs(&self) -> impl Iterator<Item = *mut GcHeader> + '_ {
        // SAFETY: what the list of them holds is allocated (only
        // `flush_unborn` drops one, after it put it on the list)
        self.puc
            .unborn
            .iter()
            .copied()
            .filter(|&h| unsafe { is_unborn(h) })
    }
}
