//! The object list and the collector's colour and phase, as the PUC model
//! (`runtime::puc`) reads and changes them: its records live on this list.

use super::*;

impl Heap {
    /// The white of objects born now (PUC `luaC_white(g)`).
    #[inline]
    pub(in crate::runtime) fn white(&self) -> u8 {
        self.current_white
    }

    /// The phase of the cycle under way.
    #[inline]
    pub(in crate::runtime) fn phase(&self) -> GcPhase {
        self.phase
    }

    /// The newest object on the list (`allgc`).
    #[inline]
    pub(in crate::runtime) fn list_head(&self) -> *mut GcHeader {
        self.all
    }

    /// The first object never swept (`fixedgc`).
    #[inline]
    pub(in crate::runtime) fn fixed_head(&self) -> *mut GcHeader {
        self.fixed
    }

    /// The heap is being dropped: nothing is counted.
    #[inline]
    pub(in crate::runtime) fn is_dropping(&self) -> bool {
        self.dropping
    }

    /// The threads the collector visits.
    #[inline]
    pub(in crate::runtime) fn threads(&self) -> &[Gc<crate::runtime::Coro>] {
        &self.threads
    }

    /// Each interned string.
    pub(in crate::runtime) fn each_string(&self, f: impl FnMut(*mut LuaStr)) {
        self.strings.each(f);
    }

    /// [`Heap::link`] onto the list.
    ///
    /// # Safety
    /// As [`Heap::link`]'s.
    #[inline]
    pub(in crate::runtime) unsafe fn link_now(&mut self, h: *mut GcHeader) {
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

    /// `h`, off the list, back at its head (a string the string sweep
    /// held and keeps).
    ///
    /// # Safety
    /// `h` heads a live object of this heap on no list.
    pub(in crate::runtime) unsafe fn relink(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe { (*h).next = self.all };
        self.all = h;
    }

    /// `h`, off the list, freed (a string the string sweep held and frees).
    ///
    /// # Safety
    /// `h` heads a dead object of this heap on no list.
    pub(in crate::runtime) unsafe fn free_unlisted(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe { self.free_obj(h) };
        self.live -= 1;
    }

    /// Move the object `h` on the list to just before `anchor`; nothing
    /// when `h` is `anchor`, is not on the list or the sweep stands at it.
    ///
    /// # Safety
    /// `h` and `anchor` head live objects of this heap.
    pub(in crate::runtime) unsafe fn move_before(
        &mut self,
        h: *mut GcHeader,
        anchor: *mut GcHeader,
    ) {
        if h == anchor || h == self.sweep_prev {
            return;
        }
        // SAFETY: the caller's contract; the links walked are the list's
        unsafe {
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
}
