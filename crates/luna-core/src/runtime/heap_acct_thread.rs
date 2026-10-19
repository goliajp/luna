//! What PUC allocates for a thread: its state, its stack and its
//! `CallInfo`s.

use super::*;
use crate::runtime::table::Dialect;

impl Heap {
    /// `new` `CallInfo`s allocated and `freed` ones freed, one block each.
    pub(crate) fn acct_ci(&mut self, new: u32, freed: u32) {
        let ci = self.sizes.callinfo;
        for _ in 0..freed {
            self.acct_block(ci, 0);
        }
        for _ in 0..new {
            self.acct_block(0, ci);
        }
    }
}

/// PUC `BASIC_CI_SIZE`: 5.1's `CallInfo` array of a new thread.
pub(crate) const BASIC_CI_51: u32 = 8;

impl Heap {
    /// The bytes of a stack of `s` slots.
    pub(in crate::runtime::heap) fn stack_bytes(&self, s: u32) -> usize {
        crate::runtime::heap::pace::stack_bytes(&self.sizes, self.table_dialect, s)
    }

    /// A thread's stack resized from `old` to `new` slots
    /// (`luaD_reallocstack`).
    pub(crate) fn acct_stack(&mut self, old: u32, new: u32) {
        if old != new {
            self.acct_block(self.stack_bytes(old), self.stack_bytes(new));
        }
    }

    /// The stack of `s` slots reallocated to the size it has.
    pub(crate) fn acct_stack_same(&mut self, s: u32) {
        let b = self.stack_bytes(s);
        self.acct_block(b, b);
    }

    /// 5.1's `CallInfo` array resized from `old` to `new` entries
    /// (`luaD_reallocCI`).
    pub(crate) fn acct_ci51(&mut self, old: u32, new: u32) {
        if old != new {
            let ci = self.sizes.callinfo;
            self.acct_block(old as usize * ci, new as usize * ci);
        }
    }

    /// What a new thread allocates after its state (`stack_init`): 5.1 its
    /// `CallInfo` array, then its stack of `stack` slots.
    pub(crate) fn acct_thread_blocks(&mut self, stack: u32) {
        if self.table_dialect == Dialect::L51 {
            self.acct_ci51(0, BASIC_CI_51);
        }
        self.acct_stack(0, stack);
    }

    /// A thread freed as `luaE_freethread` frees it: its `CallInfo`s, its
    /// stack of `stack` slots, its state. `ci` is 5.1's array size, or the
    /// `CallInfo`s a later thread allocated besides its base one.
    pub(crate) fn acct_thread_free(&mut self, stack: u32, ci: u32) {
        if self.table_dialect == Dialect::L51 {
            self.acct_ci51(ci, 0);
        } else {
            self.acct_ci(0, ci);
        }
        self.acct_stack(stack, 0);
        self.acct_free(self.sizes.thread);
    }
}
