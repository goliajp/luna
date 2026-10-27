//! Counting what PUC allocates and frees: objects, blocks, threads' stacks,
//! and the records of objects luna has none of.

use super::super::*;
use crate::runtime::heap::Heap;
use crate::runtime::mem::Oom;
use crate::runtime::puc::records::GhostRef;
use crate::runtime::puc::state::{CountingMark, CountingOff};

/// Counting what PUC allocates and frees: objects, blocks, threads' stacks,
/// and the records of objects luna has none of.
pub(crate) trait PucCount {
    /// A block (an array, a buffer) going from `old` to `new` bytes; either
    /// may be 0.
    fn acct_block(&mut self, old: usize, new: usize);
    /// `new` `CallInfo`s of `t` allocated and `freed` ones freed, one block
    /// each.
    fn acct_ci(&mut self, t: Thread, new: u32, freed: u32);
    /// 5.1's `CallInfo` array of `t` resized from `old` to `new` entries
    /// (`luaD_reallocCI`).
    fn acct_ci51(&mut self, t: Thread, old: u32, new: u32);
    /// A table PUC makes, `a` array slots and `h` nodes, and luna does not
    /// (yet): counted now, freed by the next sweep unless luna makes it
    /// after all ([`Heap::ghost_table_made`]).
    fn acct_ghost_table(&mut self, a: usize, h: usize) -> Result<GhostRef, Oom>;
    /// `cs`, a long string luna's compiler made without counting, is the
    /// one PUC's scanner made as `ss` (counted) and keeps as the constant:
    /// the count moves to `cs`.
    fn acct_move_long(&mut self, ss: Gc<LuaStr>, cs: Gc<LuaStr>);
    /// A string luna made for itself is made by the program now: PUC makes
    /// it here.
    fn acct_promote(&mut self, s: Gc<LuaStr>, len: usize);
    /// The parser's string of the chunk name `s`: counted now if luna made
    /// it without counting (PUC found it made already otherwise).
    fn acct_source(&mut self, s: Gc<LuaStr>);
    /// The stack of `t` resized from `old` to `new` slots
    /// (`luaD_reallocstack`).
    fn acct_stack(&mut self, t: Thread, old: u32, new: u32);
    /// The stack of `t`, `s` slots, reallocated to the size it has.
    fn acct_stack_same(&mut self, t: Thread, s: u32);
    /// A table's parts going from `old_a` array slots and `old_h` nodes to
    /// `new_a` and `new_h`, as PUC's `luaH_resize` allocates and frees them:
    /// up to 5.3 a growing array first, then the new nodes, a shrinking
    /// array, the old nodes; from 5.4 the new nodes, the array, the old
    /// nodes. A part of no nodes is PUC's shared dummy node, never
    /// allocated. For a table of PUC's luna has none for (a parser's,
    /// the registry PUC's state starts with): counted unless luna is
    /// building its own objects.
    fn acct_table_resize(&mut self, old: (usize, usize), new: (usize, usize));
    /// gc-trace: the count moved by `delta` where the instrumented PUC's
    /// allocation function refused a growth.
    #[cfg(feature = "gc-trace")]
    fn adjust_count(&mut self, delta: i64);
    /// `t`, made while nothing was counted, is a table PUC's state starts
    /// with, which [`Heap::acct_boot`] counted in its place: counted from
    /// here on, as it grows and when it is freed.
    fn boot_table(t: Gc<crate::runtime::Table>) -> Gc<crate::runtime::Table>;
    /// Count everything from here, also in the middle of what is not counted
    /// (an emergency collection): where it stood, to return to.
    fn count_all(&mut self) -> CountingMark;
    /// Whether what is made now is counted.
    fn counting(&self) -> bool;
    /// Stop counting until the token is handed back.
    fn counting_off(&mut self) -> CountingOff;
    /// Count again what [`Heap::counting_off`] stopped.
    fn counting_on(&mut self, off: CountingOff);
    /// luna made `h`, without counting, for the object of `r` after all
    /// (one without parts): the record neither frees nor sweeps it, and
    /// `h` is counted from now on.
    fn ghost_made(&mut self, r: GhostRef, h: Gc<GcHeader>);
    /// luna made `h`, without counting, for the table of `t` after all (a
    /// trace leaving with it): the record neither frees nor sweeps it, and
    /// `h` is counted from now on, as it grows and when it is freed (made
    /// uncounted, it was one of luna's own tables).
    fn ghost_table_made(&mut self, t: GhostRef, h: Gc<GcHeader>);
    /// The table of `t` resized from `old_a` array slots and `old_h` nodes
    /// to `new_a` and `new_h` (PUC `luaH_resize`).
    fn ghost_table_resize(
        &mut self,
        t: GhostRef,
        old_a: usize,
        old_h: usize,
        new_a: usize,
        new_h: usize,
    );
    /// The bytes PUC allocates for `o` in this dialect.
    fn puc_size(&self, o: PucObj) -> usize;
    /// PUC's bytes of a new table of `a` array slots and `h` nodes.
    fn puc_table_bytes(&self, a: usize, h: usize) -> usize;
    /// Back to where counting stood at `m`.
    fn restore_counting(&mut self, m: CountingMark);
    /// A new object `o`.
    fn new_obj(&mut self, o: PucObj);
    /// `o`, made without counting, is PUC's now: counted here.
    fn promote_obj(&mut self, h: Gc<GcHeader>, o: PucObj);
    /// An object `o` PUC makes and luna does not: counted now, freed by the
    /// next sweep.
    fn ghost_obj(&mut self, o: PucObj) -> Result<(), Oom>;
    /// An object `o` PUC makes and luna does not (yet), as
    /// [`Heap::ghost_obj`]: where its record is.
    fn ghost_obj_ref(&mut self, o: PucObj) -> Result<GhostRef, Oom>;
    /// The bytes PUC allocates for `o` in this dialect.
    fn puc_bytes(&self, o: PucObj) -> usize;
    /// `cl`, a main closure made without counting, is PUC's now: counted
    /// here, without 5.1's environment among its upvalues.
    fn promote_main_closure(&mut self, cl: Gc<LuaClosure>);
}

impl PucCount for Heap {
    #[inline(always)]
    fn acct_block(&mut self, old: usize, new: usize) {
        Heap::acct_block(self, old, new)
    }

    #[inline(always)]
    fn acct_ci(&mut self, t: Thread, new: u32, freed: u32) {
        Heap::acct_ci(self, t, new, freed)
    }

    #[inline(always)]
    fn acct_ci51(&mut self, t: Thread, old: u32, new: u32) {
        Heap::acct_ci51(self, t, old, new)
    }

    #[inline(always)]
    fn acct_ghost_table(&mut self, a: usize, h: usize) -> Result<GhostRef, Oom> {
        Heap::acct_ghost_table(self, a, h)
    }

    #[inline(always)]
    fn acct_move_long(&mut self, ss: Gc<LuaStr>, cs: Gc<LuaStr>) {
        Heap::acct_move_long(self, ss, cs)
    }

    #[inline(always)]
    fn acct_promote(&mut self, s: Gc<LuaStr>, len: usize) {
        Heap::acct_promote(self, s, len)
    }

    #[inline(always)]
    fn acct_source(&mut self, s: Gc<LuaStr>) {
        Heap::acct_source(self, s)
    }

    #[inline(always)]
    fn acct_stack(&mut self, t: Thread, old: u32, new: u32) {
        Heap::acct_stack(self, t, old, new)
    }

    #[inline(always)]
    fn acct_stack_same(&mut self, t: Thread, s: u32) {
        Heap::acct_stack_same(self, t, s)
    }

    #[inline(always)]
    fn acct_table_resize(&mut self, old: (usize, usize), new: (usize, usize)) {
        Heap::acct_table_resize(self, old, new)
    }

    #[cfg(feature = "gc-trace")]
    #[inline(always)]
    fn adjust_count(&mut self, delta: i64) {
        Heap::adjust_count(self, delta)
    }

    #[inline(always)]
    fn boot_table(t: Gc<crate::runtime::Table>) -> Gc<crate::runtime::Table> {
        Heap::boot_table(t)
    }

    #[inline(always)]
    fn count_all(&mut self) -> CountingMark {
        Heap::count_all(self)
    }

    #[inline(always)]
    fn counting(&self) -> bool {
        Heap::counting(self)
    }

    #[inline(always)]
    fn counting_off(&mut self) -> CountingOff {
        Heap::counting_off(self)
    }

    #[inline(always)]
    fn counting_on(&mut self, off: CountingOff) {
        Heap::counting_on(self, off)
    }

    #[inline(always)]
    fn ghost_made(&mut self, r: GhostRef, h: Gc<GcHeader>) {
        Heap::ghost_made(self, r, h)
    }

    #[inline(always)]
    fn ghost_table_made(&mut self, t: GhostRef, h: Gc<GcHeader>) {
        Heap::ghost_table_made(self, t, h)
    }

    #[inline(always)]
    fn ghost_table_resize(
        &mut self,
        t: GhostRef,
        old_a: usize,
        old_h: usize,
        new_a: usize,
        new_h: usize,
    ) {
        Heap::ghost_table_resize(self, t, old_a, old_h, new_a, new_h)
    }

    #[inline(always)]
    fn puc_size(&self, o: PucObj) -> usize {
        Heap::puc_size(self, o)
    }

    #[inline(always)]
    fn puc_table_bytes(&self, a: usize, h: usize) -> usize {
        Heap::puc_table_bytes(self, a, h)
    }

    #[inline(always)]
    fn restore_counting(&mut self, m: CountingMark) {
        Heap::restore_counting(self, m)
    }

    #[inline(always)]
    fn new_obj(&mut self, o: PucObj) {
        Heap::new_obj(self, o)
    }

    #[inline(always)]
    fn promote_obj(&mut self, h: Gc<GcHeader>, o: PucObj) {
        Heap::promote_obj(self, h, o)
    }

    #[inline(always)]
    fn ghost_obj(&mut self, o: PucObj) -> Result<(), Oom> {
        Heap::ghost_obj(self, o)
    }

    #[inline(always)]
    fn ghost_obj_ref(&mut self, o: PucObj) -> Result<GhostRef, Oom> {
        Heap::ghost_obj_ref(self, o)
    }

    fn puc_bytes(&self, o: PucObj) -> usize {
        Heap::puc_size(self, o)
    }

    #[inline(always)]
    fn promote_main_closure(&mut self, cl: Gc<LuaClosure>) {
        Heap::promote_main_closure(self, cl)
    }
}
