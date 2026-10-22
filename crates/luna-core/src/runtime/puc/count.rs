//! PUC's `totalbytes` and the events that change it.
//!
//! Every object and every block PUC would allocate is counted at PUC's
//! size for it, in the order PUC allocates it, so `collectgarbage("count")`
//! and the collector's pacing see what they see in PUC. luna's own objects
//! are laid out differently (and count separately, in the allocation
//! context); nothing here touches them.

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::size::PucSizes;

/// What a counted allocation is, for PUC's tag of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PucKind {
    ShortStr,
    LongStr,
    Table,
    LuaClosure,
    CClosure,
    Userdata,
    Thread,
    Proto,
    Upval,
}

impl Heap {
    /// The model takes on `version`: PUC's sizes and dialect, and 5.1's
    /// string table holding every string.
    pub(in crate::runtime) fn puc_dialect(&mut self, version: crate::version::LuaVersion) {
        let d = Dialect::of(version);
        self.puc.dialect = d;
        self.puc.params = super::GcParams::new(version);
        self.puc.sizes = PucSizes::of(d);
        if d == Dialect::L51 {
            self.puc.short_max = usize::MAX;
        }
    }

    /// A new object of `size` bytes.
    #[inline]
    pub(crate) fn acct_obj(&mut self, kind: PucKind, size: usize) {
        if !self.counting() {
            return;
        }
        self.puc.bytes += size;
        self.hook_alloc(kind, size);
    }

    /// A new object (not a table, not a string) `h` of `size` bytes:
    /// counted, or, while luna builds objects of its own, marked as one PUC
    /// has not made, whose free is not counted either (see
    /// [`Heap::acct_promote_obj`]).
    pub(crate) fn acct_new(&mut self, h: *mut GcHeader, kind: PucKind, size: usize) {
        if !self.counting() {
            set_uncounted(h, true);
        } else {
            self.acct_obj(kind, size);
        }
    }

    /// An object made without counting is PUC's now: counted here.
    pub(crate) fn acct_promote_obj(&mut self, h: *mut GcHeader, kind: PucKind, size: usize) {
        if self.counting() && is_uncounted(h) {
            // SAFETY: the caller names a live object
            unsafe { self.born(h) };
            set_uncounted(h, false);
            self.acct_obj(kind, size);
        }
    }

    /// A block (an array, a buffer) going from `old` to `new` bytes; either may be 0.
    #[inline]
    pub(crate) fn acct_block(&mut self, old: usize, new: usize) {
        if !self.counting() {
            return;
        }
        self.hook_block(old, new);
        self.puc.bytes = self.puc.bytes + new - old;
    }

    /// A block of an object PUC counts going from `old` to `new` bytes (a
    /// table's parts, a thread's stack or `CallInfo`s; `new` 0 for a free):
    /// counted as the object itself is, whatever luna builds meanwhile, so
    /// that what its free takes off is what was counted. Nothing once the
    /// heap is being dropped.
    #[inline]
    pub(crate) fn acct_part(&mut self, old: usize, new: usize) {
        if !self.is_dropping() {
            self.count_part(old, new);
        }
    }

    /// [`Heap::acct_part`] for a caller that has seen the heap is not
    /// being dropped.
    #[inline]
    pub(in crate::runtime) fn count_part(&mut self, old: usize, new: usize) {
        self.hook_block(old, new);
        self.puc.bytes = self.puc.bytes + new - old;
    }
}

/// Whether object `h` is one PUC has not made (see [`Heap::acct_new`]).
#[inline(always)]
pub(crate) fn is_uncounted(h: *const GcHeader) -> bool {
    // SAFETY: `h` heads a live object of this heap; its header is read
    unsafe { (*h).sub & UNCOUNTED != 0 }
}

/// in a string's `sub` byte: a 5.5 external string over bytes PUC does not
/// own (`LSTRFIX`), its header alone counted
pub(crate) const FIXED_EXT: u8 = 0x20;

pub(in crate::runtime) fn set_uncounted(h: *mut GcHeader, on: bool) {
    // SAFETY: `h` heads a live object of this heap; only its per-type byte
    // is written, which no type but a table uses otherwise
    unsafe {
        if on {
            (*h).sub |= UNCOUNTED;
        } else {
            (*h).sub &= !UNCOUNTED;
        }
    }
}
