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

/// PUC's count. A safe point compares `bytes` with `gc_limit`, side by
/// side here, in this order: compiled code reads the two words itself
/// (see [`GC_LIMIT_OFFSET`]).
#[repr(C)]
pub(in crate::runtime::puc) struct Count {
    /// PUC's `totalbytes`: every object and block at the size PUC gives it
    /// (`sizes`), not what luna's own objects take (see `runtime::puc`)
    pub(in crate::runtime::puc) bytes: usize,
    /// where the next collection step falls due (`pace`), or `usize::MAX`
    /// while there is none: the one comparison a safe point makes
    gc_limit: usize,
    /// above 0, nothing is counted (objects PUC made elsewhere, or not at
    /// all, are being built): changed only through the facade's
    /// `CountingOff` and `CountingMark`
    silent: u32,
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
            silent: 1,
        }
    }

    /// Whether the count reaches the due point once `more` bytes more are
    /// counted.
    #[inline(always)]
    fn due_after(&self, more: usize) -> bool {
        self.bytes + more >= self.gc_limit
    }

    /// Whether what is made now is counted.
    #[inline(always)]
    pub(in crate::runtime::puc) fn counts(&self) -> bool {
        self.silent == 0
    }

    /// Counting stopped once more.
    #[inline(always)]
    pub(in crate::runtime::puc) fn silence(&mut self) {
        self.silent += 1;
    }

    /// One stop of counting taken back.
    #[inline(always)]
    pub(in crate::runtime::puc) fn unsilence(&mut self) {
        self.silent -= 1;
    }

    /// How many stops of counting there are.
    #[inline]
    pub(in crate::runtime::puc) fn silent_level(&self) -> u32 {
        self.silent
    }

    /// Counting stopped `n` times, as some mark recorded; the level before.
    #[inline]
    pub(in crate::runtime::puc) fn set_silent_level(&mut self, n: u32) -> u32 {
        std::mem::replace(&mut self.silent, n)
    }
}

/// What a counted allocation is, for PUC's tag of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::runtime) enum PucKind {
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
    pub(in crate::runtime::puc) fn puc_dialect(&mut self, version: crate::version::LuaVersion) {
        let d = Dialect::of(version);
        self.puc.dialect = d;
        self.puc.params = super::GcParams::new(version);
        self.puc.sizes = PucSizes::of(d);
        self.puc.rules = super::for_heap::HeapRules::of(d, &self.puc.sizes);
        if d == Dialect::L51 {
            self.puc.records.short_max = usize::MAX;
        }
    }

    /// A new object of `size` bytes.
    #[inline]
    pub(in crate::runtime::puc) fn acct_obj(&mut self, kind: PucKind, size: usize) {
        if !self.counting() {
            return;
        }
        self.puc.count.bytes += size;
        self.hook_alloc(kind, size);
    }

    /// A new object (not a table, not a string) `h` of `size` bytes:
    /// counted, or, while luna builds objects of its own, marked as one PUC
    /// has not made, whose free is not counted either (see
    /// [`Heap::acct_promote_obj`]).
    pub(in crate::runtime::puc) fn acct_new(
        &mut self,
        h: Gc<GcHeader>,
        kind: PucKind,
        size: usize,
    ) {
        if !self.counting() {
            // SAFETY: `h` is a handle, so its object is live; nothing
            // borrows the header while its per-type byte is written
            unsafe { (*h.as_ptr()).set_uncounted(true) };
        } else {
            self.acct_obj(kind, size);
        }
    }

    /// An object made without counting is PUC's now: counted here.
    pub(in crate::runtime::puc) fn acct_promote_obj(
        &mut self,
        h: Gc<GcHeader>,
        kind: PucKind,
        size: usize,
    ) {
        if self.counting() && h.is_uncounted() {
            // SAFETY: `h` is a handle, so its object is live; nothing
            // borrows the header while its per-type byte is written
            unsafe {
                self.born(h.as_ptr());
                (*h.as_ptr()).set_uncounted(false);
            }
            self.acct_obj(kind, size);
        }
    }

    /// A block of an object PUC counts going from `old` to `new` bytes (a
    /// table's parts, a thread's stack or `CallInfo`s; `new` 0 for a free):
    /// counted as the object itself is, whatever luna builds meanwhile, so
    /// that what its free takes off is what was counted. Nothing once the
    /// heap is being dropped.
    #[inline]
    pub(in crate::runtime::puc) fn acct_part(&mut self, old: usize, new: usize) {
        if !self.is_dropping() {
            self.count_part(old, new);
        }
    }

    /// [`Heap::acct_part`] for a caller that has seen the heap is not
    /// being dropped.
    #[inline]
    pub(in crate::runtime::puc) fn count_part(&mut self, old: usize, new: usize) {
        self.hook_block(old, new);
        self.puc.count.bytes = self.puc.count.bytes + new - old;
    }
}

/// in a string's `sub` byte: a 5.5 external string over bytes PUC does not
/// own (`LSTRFIX`), its header alone counted
pub(in crate::runtime::puc) const FIXED_EXT: u8 = 0x20;

impl GcHeader {
    /// Marks this object as one PUC has (not) made; only the per-type byte
    /// is written, which no type but a table uses otherwise.
    #[inline]
    pub(in crate::runtime::puc) fn set_uncounted(&mut self, on: bool) {
        if on {
            self.sub |= UNCOUNTED;
        } else {
            self.sub &= !UNCOUNTED;
        }
    }
}

impl Heap {
    /// Whether the count reaches the next step's due point once `more`
    /// bytes more are counted.
    #[inline(always)]
    pub(in crate::runtime::puc) fn puc_due_after(&self, more: usize) -> bool {
        self.puc.count.due_after(more)
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
}
