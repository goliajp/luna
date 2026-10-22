//! What PUC allocates for a thread: its state, its stack and its
//! `CallInfo`s.

use super::*;
use crate::runtime::table::Dialect;

impl Heap {
    /// `new` `CallInfo`s allocated and `freed` ones freed, one block each,
    /// of a thread `counted` or luna's own (see [`Heap::acct_part`]).
    pub(crate) fn acct_ci(&mut self, counted: bool, new: u32, freed: u32) {
        if !counted {
            return;
        }
        let ci = self.sizes.callinfo;
        for _ in 0..freed {
            self.acct_part(ci, 0);
        }
        for _ in 0..new {
            self.acct_part(0, ci);
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
    /// (`luaD_reallocstack`), the thread `counted` or luna's own.
    pub(crate) fn acct_stack(&mut self, counted: bool, old: u32, new: u32) {
        if counted && old != new {
            self.acct_part(self.stack_bytes(old), self.stack_bytes(new));
        }
    }

    /// The stack of `s` slots reallocated to the size it has.
    pub(crate) fn acct_stack_same(&mut self, counted: bool, s: u32) {
        if counted {
            let b = self.stack_bytes(s);
            self.acct_part(b, b);
        }
    }

    /// 5.1's `CallInfo` array resized from `old` to `new` entries
    /// (`luaD_reallocCI`), the thread `counted` or luna's own.
    pub(crate) fn acct_ci51(&mut self, counted: bool, old: u32, new: u32) {
        if counted && old != new {
            let ci = self.sizes.callinfo;
            self.acct_part(old as usize * ci, new as usize * ci);
        }
    }

    /// What a new thread allocates after its state (`stack_init`): 5.1 its
    /// `CallInfo` array, then its stack of `stack` slots.
    pub(crate) fn acct_thread_blocks(&mut self, stack: u32) {
        // counted as the thread is: not while luna builds its own objects
        let counted = self.puc.silent == 0;
        if self.table_dialect == Dialect::L51 {
            self.acct_ci51(counted, 0, BASIC_CI_51);
        }
        self.acct_stack(counted, 0, stack);
    }

    /// A thread freed as `luaE_freethread` frees it: its `CallInfo`s, its
    /// stack of `stack` slots, its state. `ci` is 5.1's array size, or the
    /// `CallInfo`s a later thread allocated besides its base one.
    pub(crate) fn acct_thread_free(&mut self, stack: u32, ci: u32) {
        if self.table_dialect == Dialect::L51 {
            self.acct_ci51(true, ci, 0);
        } else {
            self.acct_ci(true, 0, ci);
        }
        self.acct_stack(true, stack, 0);
        self.acct_part(self.sizes.thread, 0);
    }
}
