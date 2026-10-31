//! Counting what PUC allocates and frees: objects, blocks, threads' stacks,
//! and the records of objects luna has none of.

use super::super::*;
use crate::runtime::heap::UNLISTED;
use crate::runtime::mem::Oom;
use crate::runtime::puc::records::GhostRef;

#[cfg(feature = "gc-trace")]
mod trace;

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
    /// Where counting stands now, to return to with [`Heap::restore_counting`].
    #[inline]
    pub(in crate::runtime::puc) fn counting_mark(&self) -> CountingMark {
        CountingMark(self.puc.count.silent_level())
    }

    /// A block (an array, a buffer) going from `old` to `new` bytes; either may be 0.
    #[inline]
    pub(crate) fn acct_block(&mut self, old: usize, new: usize) {
        if !self.counting() {
            return;
        }
        self.hook_block(old, new);
        self.puc.count.bytes = self.puc.count.bytes + new - old;
    }

    /// `new` `CallInfo`s of `t` allocated and `freed` ones freed, one block
    /// each.
    pub(crate) fn acct_ci(&mut self, t: Thread, new: u32, freed: u32) {
        if t.counted() {
            self.ci_blocks(new, freed);
        }
    }

    /// The stack of `t` resized from `old` to `new` slots
    /// (`luaD_reallocstack`).
    pub(crate) fn acct_stack(&mut self, t: Thread, old: u32, new: u32) {
        if t.counted() {
            self.stack_blocks(old, new);
        }
    }

    /// The stack of `t`, `s` slots, reallocated to the size it has.
    pub(crate) fn acct_stack_same(&mut self, t: Thread, s: u32) {
        if t.counted() {
            let b = self.stack_bytes(s);
            self.acct_part(b, b);
        }
    }

    /// 5.1's `CallInfo` array of `t` resized from `old` to `new` entries
    /// (`luaD_reallocCI`).
    pub(crate) fn acct_ci51(&mut self, t: Thread, old: u32, new: u32) {
        if t.counted() {
            self.ci51_blocks(old, new);
        }
    }

    /// A table's parts going from `old_a` array slots and `old_h` nodes to
    /// `new_a` and `new_h`, as PUC's `luaH_resize` allocates and frees them:
    /// up to 5.3 a growing array first, then the new nodes, a shrinking
    /// array, the old nodes; from 5.4 the new nodes, the array, the old
    /// nodes. A part of no nodes is PUC's shared dummy node, never
    /// allocated. For a table of PUC's luna has none for (a parser's,
    /// the registry PUC's state starts with): counted unless luna is
    /// building its own objects.
    pub(crate) fn acct_table_resize(&mut self, old: (usize, usize), new: (usize, usize)) {
        if self.counting() {
            self.table_parts(old, new, Heap::acct_block);
        }
    }

    /// `t`, made while nothing was counted, is a table PUC's state starts
    /// with, which [`Heap::acct_boot`] counted in its place: counted from
    /// here on, as it grows and when it is freed.
    pub(crate) fn boot_table(t: Gc<crate::runtime::Table>) -> Gc<crate::runtime::Table> {
        // SAFETY: `t` is a live table; only its class byte is written
        unsafe { (*t.header()).cls &= !UNLISTED };
        t
    }

    /// Whether what is made now is counted.
    #[inline]
    pub(crate) fn counting(&self) -> bool {
        self.puc.count.counts()
    }

    /// Stop counting until the token is handed back.
    #[inline]
    pub(crate) fn counting_off(&mut self) -> CountingOff {
        self.puc.count.silence();
        CountingOff { back: false }
    }

    /// Count again what [`Heap::counting_off`] stopped.
    #[inline]
    pub(crate) fn counting_on(&mut self, mut off: CountingOff) {
        off.back = true;
        self.puc.count.unsilence();
    }

    /// Count everything from here, also in the middle of what is not counted
    /// (an emergency collection): where it stood, to return to.
    #[inline]
    pub(crate) fn count_all(&mut self) -> CountingMark {
        CountingMark(self.puc.count.set_silent_level(0))
    }

    /// Back to where counting stood at `m`.
    #[inline]
    pub(crate) fn restore_counting(&mut self, m: CountingMark) {
        self.puc.count.set_silent_level(m.0);
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

impl GcHeader {
    /// Whether this object is one PUC has not made (see [`Heap::acct_new`]).
    #[inline(always)]
    pub(crate) fn is_uncounted(&self) -> bool {
        self.sub & UNCOUNTED != 0
    }
}
