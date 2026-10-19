//! What a load makes before PUC does: luna compiles a chunk before it
//! counts it, so the objects the compiler makes are kept off the list of
//! objects until the count reaches the point where PUC makes each one,
//! which puts it on the list where PUC's is (the sweep goes over the list
//! in that order).

use super::*;

/// In the `next` of an object set aside: PUC has not made it yet.
const UNBORN: *mut GcHeader = ptr::without_provenance_mut(1);

impl Heap {
    /// [`Heap::link`] onto the list.
    ///
    /// # Safety
    /// As [`Heap::link`]'s.
    #[inline]
    pub(in crate::runtime::heap) unsafe fn link_now(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            (*h).next = self.all;
            // born the current white in every phase (PUC `luaC_white(g)`):
            // one made while marking is marked by the atomic step if it is
            // reachable then (the roots are marked again there, and a black
            // object that took it was sent back to gray by its barrier)
            (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | self.current_white);
        }
        self.all = h;
        self.live += 1;
        #[cfg(feature = "gc-verify")]
        self.forget_freed(h);
    }

    /// A new object a load makes before PUC does (luna compiles first),
    /// kept off the list until PUC makes it, which puts it where PUC's is.
    /// False when there is no room to note it (it goes on the list now).
    ///
    /// # Safety
    /// As [`Heap::link`]'s.
    #[cold]
    pub(in crate::runtime::heap) unsafe fn set_aside(&mut self, h: *mut GcHeader) -> bool {
        if self.puc.unborn.try_reserve(1).is_err() {
            return false;
        }
        self.puc.unborn.push(h);
        // SAFETY: the caller's contract
        unsafe {
            (*h).next = UNBORN;
            // white as a new object, for the barriers that reach it first
            (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | self.current_white);
        }
        true
    }

    /// PUC makes the object `h` now: on the list if it was set aside.
    ///
    /// # Safety
    /// `h` heads a live object of this heap.
    #[inline]
    pub(in crate::runtime::heap) unsafe fn born(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe {
            if (*h).next != UNBORN {
                return;
            }
            // one a marking of this cycle made black already (luna holds it
            // before PUC makes it) stays black: what holds it may be black
            // too (one gray is on a gray stack, traversed whatever its color)
            let f = (*h).flags;
            self.link_now(h);
            if self.phase == GcPhase::Propagate && is_black(f) {
                (*h).flags = f;
            }
        }
    }

    /// Set what a load makes before PUC does aside (`on`), or no more.
    pub(crate) fn set_aside_loads(&mut self, on: bool) {
        self.puc.unborn_on = on;
    }

    /// The objects set aside since `base` (a length of the list of them)
    /// that PUC did not make go on the list: luna's own, or garbage of a
    /// load that failed.
    pub(crate) fn flush_unborn(&mut self, base: usize) {
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
        if h == anchor || h == self.sweep_prev {
            return;
        }
        // SAFETY: the caller's contract; the links walked are the list's
        unsafe {
            if (*h).next == UNBORN || (*anchor).next == UNBORN {
                return;
            }
            let mut link: *mut *mut GcHeader = ptr::addr_of_mut!(self.all);
            while !(*link).is_null() && *link != h {
                link = ptr::addr_of_mut!((**link).next);
            }
            if (*link).is_null() {
                return;
            }
            *link = (*h).next;
            let mut at: *mut *mut GcHeader = ptr::addr_of_mut!(self.all);
            while !(*at).is_null() && *at != anchor {
                at = ptr::addr_of_mut!((**at).next);
            }
            if (*at).is_null() {
                // not found after all: back at the head
                (*h).next = self.all;
                self.all = h;
                return;
            }
            (*h).next = anchor;
            *at = h;
        }
    }

    /// How many objects are set aside, for [`Heap::flush_unborn`].
    pub(crate) fn unborn_len(&self) -> usize {
        self.puc.unborn.len()
    }

    /// A sweep is over: what is set aside is white for the next cycle, as
    /// the sweep leaves every object it goes over (a marking of the cycle
    /// that ended may have reached it).
    pub(in crate::runtime::heap) fn whiten_unborn(&mut self) {
        let white = self.current_white;
        for &h in &self.puc.unborn {
            // SAFETY: an object set aside is allocated; only its flags are
            // written
            unsafe {
                if (*h).next == UNBORN {
                    (*h).flags = (*h).with_slow(((*h).flags & !COLOR_BITS) | white);
                }
            }
        }
    }
}
