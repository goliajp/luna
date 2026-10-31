//! Counting the objects PUC makes: at PUC's size, when PUC makes them, and
//! the objects luna makes that PUC has not (yet).

use super::super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::mem::Oom;
use crate::runtime::puc::records::GhostRef;

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

/// in a string's `sub` byte: a 5.5 external string over bytes PUC does not
/// own (`LSTRFIX`), its header alone counted
pub(in crate::runtime) const FIXED_EXT: u8 = 0x20;

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

    /// Whether this object is one PUC has not made (see [`Heap::acct_new`]).
    #[inline(always)]
    pub(crate) fn is_uncounted(&self) -> bool {
        self.sub & UNCOUNTED != 0
    }
}

impl Heap {
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

    /// A new string of `len` bytes (PUC `createstrobj`).
    #[inline(always)]
    pub(in crate::runtime::puc) fn acct_string(&mut self, len: usize) {
        // a short string with no trace to write: only the sum is seen
        // (the table grows on the same condition either side of the
        // string's own count)
        if !crate::runtime::gc_hooks::TRACE && self.counting() {
            if len > self.puc.records.short_max {
                let l55 = self.puc.dialect == Dialect::L55;
                self.puc.count.bytes += self.puc.sizes.string(len, l55);
                return;
            }
            if self.puc.records.strt_nuse >= self.puc.records.strt_size {
                self.grow_strtab();
            }
            self.puc.count.bytes += self.puc.sizes.string(len, false);
            self.puc.records.strt_nuse += 1;
            return;
        }
        self.acct_string_traced(len);
    }

    /// [`Heap::acct_string`] with a trace to write or while luna builds
    /// its own objects.
    #[inline(never)]
    fn acct_string_traced(&mut self, len: usize) {
        let d = self.puc.dialect;
        let long = len > crate::runtime::string::MAX_SHORT_LEN && d != Dialect::L51;
        let kind = if long {
            PucKind::LongStr
        } else {
            PucKind::ShortStr
        };
        let size = self.puc.sizes.string(len, long && d == Dialect::L55);
        if long || !self.counting() {
            return self.acct_obj(kind, size);
        }
        // the string table grows when it is full: 5.2 on before the new
        // string goes in, 5.1 once it is past full
        if d != Dialect::L51 && self.puc.records.strt_nuse >= self.puc.records.strt_size {
            self.grow_strtab();
        }
        self.acct_obj(kind, size);
        self.puc.records.strt_nuse += 1;
        if d == Dialect::L51 && self.puc.records.strt_nuse > self.puc.records.strt_size {
            self.grow_strtab();
        }
    }

    /// A new object `o`.
    #[inline(always)]
    pub(crate) fn new_obj(&mut self, o: PucObj) {
        self.acct_obj(o.kind(), self.puc_size(o));
    }

    /// `o`, made without counting, is PUC's now: counted here.
    #[inline(always)]
    pub(crate) fn promote_obj(&mut self, h: Gc<GcHeader>, o: PucObj) {
        self.acct_promote_obj(h, o.kind(), self.puc_size(o));
    }

    /// An object `o` PUC makes and luna does not: counted now, freed by the
    /// next sweep.
    pub(crate) fn ghost_obj(&mut self, o: PucObj) -> Result<(), Oom> {
        self.acct_ghost(o.kind(), self.puc_size(o))
    }

    /// An object `o` PUC makes and luna does not (yet), as
    /// [`Heap::ghost_obj`]: where its record is.
    pub(crate) fn ghost_obj_ref(&mut self, o: PucObj) -> Result<GhostRef, Oom> {
        self.acct_ghost_ref(o.kind(), self.puc_size(o))
    }

    /// `cl`, a main closure made without counting, is PUC's now: counted
    /// here, without 5.1's environment among its upvalues.
    pub(crate) fn promote_main_closure(&mut self, cl: Gc<LuaClosure>) {
        let size = self.closure_size(&cl.proto, cl.upvals().len());
        self.acct_promote_obj(cl.hdr(), PucKind::LuaClosure, size);
    }

    /// The bytes PUC allocates for `o` in this dialect.
    #[inline]
    pub(crate) fn puc_size(&self, o: PucObj) -> usize {
        o.bytes(self.puc.dialect, &self.puc.sizes)
    }

    /// `cs`, a long string luna's compiler made without counting, is the
    /// one PUC's scanner made as `ss` (counted) and keeps as the constant:
    /// the count moves to `cs`.
    pub(crate) fn acct_move_long(&mut self, ss: Gc<LuaStr>, cs: Gc<LuaStr>) {
        if !ss.hdr().is_uncounted() && cs.hdr().is_uncounted() {
            // SAFETY: both are handles, so their objects are live; nothing
            // borrows the headers while their per-type bytes are written
            unsafe {
                (*ss.header()).set_uncounted(true);
                (*cs.header()).set_uncounted(false);
            }
        }
    }

    /// A string luna made for itself is made by the program now: PUC makes
    /// it here.
    #[cold]
    #[inline(never)]
    pub(crate) fn acct_promote(&mut self, s: Gc<LuaStr>, len: usize) {
        let h = s.header();
        if !self.counting() {
            return;
        }
        // SAFETY: `h` comes from a handle, so its string is live; nothing
        // borrows the header while its per-type byte is written
        unsafe {
            self.born(h);
            (*h).set_uncounted(false);
        }
        self.acct_string(len);
        self.strt_enter(s);
    }

    /// The parser's string of the chunk name `s`: counted now if luna made
    /// it without counting (PUC found it made already otherwise).
    pub(crate) fn acct_source(&mut self, s: Gc<LuaStr>) {
        if s.hdr.sub & UNCOUNTED != 0 {
            self.acct_promote(s, s.len());
        }
    }
}
