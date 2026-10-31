//! PUC's `totalbytes`, where the next collection step falls due, and
//! whether what is made now is counted.

use super::super::*;

/// PUC's count. A safe point compares `bytes` with `gc_limit`, side by
/// side here, in this order: compiled code reads the two words itself
/// (see [`GC_LIMIT_OFFSET`]).
#[repr(C)]
pub(in crate::runtime::puc) struct Count {
    /// PUC's `totalbytes`: every object and block at the size PUC gives it
    /// (`sizes`), not what luna's own objects take (see `runtime::puc`)
    pub(super) bytes: usize,
    /// where the next collection step falls due (`pace`), or `usize::MAX`
    /// while there is none: the one comparison a safe point makes
    gc_limit: usize,
    /// what loads' readers counted that is set aside and not counted back
    /// yet (`Heap::reader_call_begins`)
    pub(super) set_aside: isize,
    /// above 0, nothing is counted (objects PUC made elsewhere, or not at
    /// all, are being built): changed only through `CountingOff` and
    /// `CountingMark`
    silent: u32,
    /// loads set objects aside now (`records::parse::unborn`), read where
    /// every new object is linked; in the word `silent` leaves
    pub(super) unborn_on: bool,
}

/// Where `gc_limit` lies from `bytes` in [`Count`].
pub(crate) const GC_LIMIT_OFFSET: usize = std::mem::offset_of!(Count, gc_limit);

impl Count {
    /// Nothing counted, and nothing counted until the dialect is known
    /// ([`Heap::counting_begins`]).
    pub(in crate::runtime::puc) const fn new() -> Count {
        Count {
            bytes: 0,
            gc_limit: usize::MAX,
            set_aside: 0,
            silent: 1,
            unborn_on: false,
        }
    }
}

/// Counting stopped once (luna builds objects PUC made elsewhere, or not
/// at all), until [`Heap::counting_on`] takes the token back. Only
/// [`Heap::counting_off`] makes one. A token dropped any other way is
/// reported as the load tokens are (see `load_session`), unless
/// [`CountingOff::abandon`] says counting stays off for good.
#[must_use = "counting stays off until the token goes back to `Heap::counting_on`"]
pub(crate) struct CountingOff {
    back: bool,
}

impl CountingOff {
    /// Counting stays off: the heap goes with it.
    pub(crate) fn abandon(mut self) {
        self.back = true;
    }
}

impl Drop for CountingOff {
    fn drop(&mut self) {
        if !self.back {
            super::unpaired("counting turned off");
        }
    }
}

/// How far counting was off at some point, for a caller to return
/// to it whatever happened since (a load unwound by running out of memory).
#[must_use]
pub(crate) struct CountingMark(u32);

impl Heap {
    /// Approximate heap size in bytes: PUC's `totalbytes`.
    #[inline(always)]
    pub fn bytes(&self) -> usize {
        self.puc.count.bytes
    }

    /// The count moved by `delta` where the instrumented PUC's allocation
    /// function refused a growth (gc-trace's `LUNA_GC_REFUSE`).
    #[cfg(feature = "gc-trace")]
    pub(crate) fn adjust_count(&mut self, delta: i64) {
        self.puc.count.bytes = (self.puc.count.bytes as i64 + delta) as usize;
    }

    /// The two words a safe point compares, `bytes` and `gc_limit`
    /// [`GC_LIMIT_OFFSET`] bytes after it, for compiled code to compare
    /// itself: no step is due while `bytes < gc_limit`.
    pub(crate) fn due_words(&self) -> *const usize {
        std::ptr::from_ref(&self.puc.count).cast()
    }

    /// Whether the count reaches the next step's due point once `more`
    /// bytes more are counted.
    #[inline(always)]
    pub(in crate::runtime::puc) fn puc_due_after(&self, more: usize) -> bool {
        self.puc.count.bytes + more >= self.puc.count.gc_limit
    }

    /// Where the next step falls due, from the pace; never while 5.1's
    /// collector is `stopped_51` (an unreachable threshold).
    #[inline]
    pub(in crate::runtime::puc) fn puc_set_limit(&mut self, stopped_51: bool) {
        let limit = if stopped_51 {
            usize::MAX
        } else {
            self.puc.pace.limit(self.puc.dialect)
        };
        self.puc.count.gc_limit = limit;
    }

    /// The dialect is known: what is made from here on is counted
    /// ([`Heap::acct_boot`]).
    pub(in crate::runtime::puc) fn counting_begins(&mut self) {
        self.puc.count.silent = 0;
    }

    /// Whether what is made now is counted.
    #[inline]
    pub(crate) fn counting(&self) -> bool {
        self.puc.count.silent == 0
    }

    /// Stop counting until the token is handed back.
    #[inline]
    pub(crate) fn counting_off(&mut self) -> CountingOff {
        self.puc.count.silent += 1;
        CountingOff { back: false }
    }

    /// Count again what [`Heap::counting_off`] stopped.
    #[inline]
    pub(crate) fn counting_on(&mut self, mut off: CountingOff) {
        off.back = true;
        self.puc.count.silent -= 1;
    }

    /// Where counting stands now, to return to with [`Heap::restore_counting`].
    #[inline]
    pub(in crate::runtime::puc) fn counting_mark(&self) -> CountingMark {
        CountingMark(self.puc.count.silent)
    }

    /// Count everything from here, also in the middle of what is not counted
    /// (an emergency collection): where it stood, to return to.
    #[inline]
    pub(crate) fn count_all(&mut self) -> CountingMark {
        CountingMark(std::mem::replace(&mut self.puc.count.silent, 0))
    }

    /// Back to where counting stood at `m`.
    #[inline]
    pub(crate) fn restore_counting(&mut self, m: CountingMark) {
        self.puc.count.silent = m.0;
    }
}
