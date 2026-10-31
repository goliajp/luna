//! Counting the blocks of PUC's objects: arrays and buffers, a table's
//! parts, a thread's stack and `CallInfo`s.

use super::super::*;
use crate::runtime::heap::UNLISTED;

impl Heap {
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
}
