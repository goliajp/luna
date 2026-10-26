//! What a load makes before PUC does: luna compiles a chunk before it
//! counts it, so the objects the compiler makes are kept off the list of
//! objects until the count reaches the point where PUC makes each one,
//! which puts it on the list where PUC's is (the sweep goes over the list
//! in that order).

use super::*;

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
    #[inline(always)]
    pub(in crate::runtime) unsafe fn set_aside(&mut self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        self.puc.loads.unborn_on && unsafe { self.set_aside_now(h) }
    }

    /// [`Heap::set_aside`] while loads set objects aside.
    ///
    /// # Safety
    /// As [`Heap::link`]'s.
    #[cold]
    unsafe fn set_aside_now(&mut self, h: *mut GcHeader) -> bool {
        if self.puc.loads.unborn.try_reserve(1).is_err() {
            return false;
        }
        self.puc.loads.unborn.push(h);
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
    pub(in crate::runtime) unsafe fn born(&mut self, h: *mut GcHeader) {
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
            self.link_now(h);
            if self.phase() == GcPhase::Propagate && is_black(f) {
                (*h).set_flags(f);
            }
        }
    }

    /// Set what a load makes before PUC does aside (`on`), or no more,
    /// until the token goes back to [`Heap::loads_aside_back`].
    #[inline]
    pub(in crate::runtime::puc) fn set_loads_aside(&mut self, on: bool) -> LoadsAside {
        LoadsAside(std::mem::replace(&mut self.puc.loads.unborn_on, on))
    }

    /// Where loads were set aside at some point (a load unwound by running
    /// out of memory goes back there).
    #[inline]
    pub(in crate::runtime::puc) fn loads_aside_mark(&self) -> LoadsAside {
        LoadsAside(self.puc.loads.unborn_on)
    }

    /// Back to what [`Heap::set_loads_aside`] changed.
    #[inline]
    pub(in crate::runtime::puc) fn loads_aside_back(&mut self, a: LoadsAside) {
        self.puc.loads.unborn_on = a.0;
    }

    /// The objects set aside at `from..to` of the list of them that are
    /// still aside go on the list, the others staying where they are.
    pub(in crate::runtime::puc) fn born_range(&mut self, (from, to): (usize, usize)) {
        let to = to.min(self.puc.loads.unborn.len());
        for i in from.min(to)..to {
            let h = self.puc.loads.unborn[i];
            // SAFETY: an object set aside is allocated; `born` leaves one
            // already on the list alone
            unsafe { self.born(h) };
        }
    }

    /// The objects set aside since `base` (a length of the list of them)
    /// that PUC did not make go on the list: luna's own, or garbage of a
    /// load that failed.
    pub(in crate::runtime::puc) fn flush_unborn(&mut self, base: usize) {
        let mut rest = std::mem::take(&mut self.puc.loads.unborn);
        for &h in &rest[base.min(rest.len())..] {
            // SAFETY: an object set aside is allocated, and only here
            unsafe { self.born(h) };
        }
        rest.truncate(base);
        self.puc.loads.unborn = rest;
    }

    /// How many objects are set aside, for [`Heap::flush_unborn`].
    pub(in crate::runtime::puc) fn unborn_len(&self) -> usize {
        self.puc.loads.unborn.len()
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

    /// luna's name `bytes`, which PUC makes only where the program first
    /// uses it (`made_later`), and makes again after its sweep freed it:
    /// from 5.3 on, whose sweep goes over strings in the list's order, made
    /// set aside, among the first objects set aside (which no load's
    /// setting aside reaches), so that PUC making it puts it at the list's
    /// head, as PUC's is. See [`Heap::name_aside_again`].
    pub(crate) fn try_intern_name(
        &mut self,
        bytes: &[u8],
        made_later: bool,
    ) -> Result<Gc<crate::runtime::LuaStr>, crate::runtime::mem::Oom> {
        if !made_later || self.puc.rules.sweeps_string_table {
            return self.try_intern(bytes);
        }
        let a = self.set_loads_aside(true);
        let s = self.try_intern(bytes);
        self.loads_aside_back(a);
        self.puc.loads.names_aside = self.puc.loads.unborn.len();
        s
    }

    /// Whether `h`, a string PUC's sweep frees and luna keeps, is one of
    /// luna's names set aside at first: the sweep sets it aside again,
    /// for PUC's making it again to put it at the list's head.
    pub(in crate::runtime::puc) fn name_aside_again(&self, h: *mut GcHeader) -> bool {
        self.puc.loads.unborn[..self.puc.loads.names_aside].contains(&h)
    }

    /// The objects still set aside: allocated, off the lists.
    pub(in crate::runtime::puc) fn unborn_objs(&self) -> impl Iterator<Item = *mut GcHeader> + '_ {
        // SAFETY: what the list of them holds is allocated (only
        // `flush_unborn` drops one, after it put it on the list)
        self.puc
            .loads
            .unborn
            .iter()
            .copied()
            .filter(|&h| unsafe { Heap::is_detached(h) })
    }
}
