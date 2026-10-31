//! What loads make before PUC does, set aside until PUC makes it: whether
//! that is happening now, which every new object's link reads.

use super::super::*;

/// Whether loads were set aside before (see [`Heap::set_loads_aside`]).
#[must_use = "loads stay as set until the token goes back to `Heap::loads_aside_back`"]
pub(in crate::runtime::puc) struct LoadsAside(bool);

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
        self.puc.count.unborn_on && unsafe { self.set_aside_now(h) }
    }

    /// Set what a load makes before PUC does aside (`on`), or no more,
    /// until the token goes back to [`Heap::loads_aside_back`].
    #[inline]
    pub(in crate::runtime::puc) fn set_loads_aside(&mut self, on: bool) -> LoadsAside {
        LoadsAside(std::mem::replace(&mut self.puc.count.unborn_on, on))
    }

    /// Where loads were set aside at some point (a load unwound by running
    /// out of memory goes back there).
    #[inline]
    pub(in crate::runtime::puc) fn loads_aside_mark(&self) -> LoadsAside {
        LoadsAside(self.puc.count.unborn_on)
    }

    /// Back to what [`Heap::set_loads_aside`] changed.
    #[inline]
    pub(in crate::runtime::puc) fn loads_aside_back(&mut self, a: LoadsAside) {
        self.puc.count.unborn_on = a.0;
    }
}
