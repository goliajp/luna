//! What PUC allocates for a thread: its state, its stack and its
//! `CallInfo`s.

use super::*;
use crate::runtime::table::Dialect;

/// Whose stack or `CallInfo`s: the main thread, counted with the state,
/// or a coroutine, counted as the coroutine itself is.
#[derive(Clone, Copy)]
pub(crate) enum Thread {
    Main,
    Coro(Gc<crate::runtime::Coro>),
}

impl Thread {
    #[inline]
    fn counted(self) -> bool {
        match self {
            Thread::Main => true,
            Thread::Coro(c) => !is_uncounted(c.as_ptr() as *mut GcHeader),
        }
    }
}

impl Heap {
    /// `new` `CallInfo`s of `t` allocated and `freed` ones freed, one block
    /// each.
    pub(crate) fn acct_ci(&mut self, t: Thread, new: u32, freed: u32) {
        if t.counted() {
            self.ci_blocks(new, freed);
        }
    }

    fn ci_blocks(&mut self, new: u32, freed: u32) {
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
    pub(in crate::runtime) fn stack_bytes(&self, s: u32) -> usize {
        crate::runtime::puc::work::stack_bytes(&self.sizes, self.table_dialect, s)
    }

    /// The stack of `t` resized from `old` to `new` slots
    /// (`luaD_reallocstack`).
    pub(crate) fn acct_stack(&mut self, t: Thread, old: u32, new: u32) {
        if t.counted() {
            self.stack_blocks(old, new);
        }
    }

    fn stack_blocks(&mut self, old: u32, new: u32) {
        if old != new {
            self.acct_part(self.stack_bytes(old), self.stack_bytes(new));
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

    fn ci51_blocks(&mut self, old: u32, new: u32) {
        if old != new {
            let ci = self.sizes.callinfo;
            self.acct_part(old as usize * ci, new as usize * ci);
        }
    }

    /// What a new thread allocates after its state (`stack_init`): 5.1 its
    /// `CallInfo` array, then its stack of `stack` slots; counted as the
    /// thread is, not while luna builds its own objects.
    pub(crate) fn acct_thread_blocks(&mut self, stack: u32) {
        if !self.counting() {
            return;
        }
        if self.table_dialect == Dialect::L51 {
            self.ci51_blocks(0, BASIC_CI_51);
        }
        self.stack_blocks(0, stack);
    }

    /// A counted thread freed as `luaE_freethread` frees it: its
    /// `CallInfo`s, its stack of `stack` slots, its state. `ci` is 5.1's
    /// array size, or the `CallInfo`s a later thread allocated besides its
    /// base one.
    pub(crate) fn acct_thread_free(&mut self, stack: u32, ci: u32) {
        if self.table_dialect == Dialect::L51 {
            self.ci51_blocks(ci, 0);
        } else {
            self.ci_blocks(0, ci);
        }
        self.stack_blocks(stack, 0);
        self.acct_part(self.sizes.thread, 0);
    }
}
