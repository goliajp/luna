//! The words of a Vm that compiled code reads where it would otherwise call
//! into the Vm: whether the exit it is at left something to the dispatcher,
//! where the running trace's frame is on the stack, which trace runs, and
//! the links taken in a row (see `crate::vm::exec::trace::relink`).
//!
//! Each Vm has one, boxed so that its address holds while the Vm moves; the
//! code finds it through a relocation each Vm writes its own address into.
//! The Vm writes the words on one thread at a time, between the code's
//! reads, as for the side cells.

use crate::jit::send_compat::{TCellBool, TCellPtr, TCellU32};
use crate::runtime::mem::{LVec, MemRef, Oom};

mod trail;

/// See the module comment. The code reads the fields at the offsets
/// [`RunWords::offsets`] gives.
#[repr(C)]
pub struct RunWords {
    /// Read as one 32-bit word, non-zero while an exit left something for
    /// the dispatcher to do: a collection check ([`BLOCKED_GC_EXIT`]), an
    /// error to raise ([`BLOCKED_PENDING_ERR`]), something to make
    /// ([`BLOCKED_UNMADE`]); one byte each.
    pub(crate) blocked: [TCellBool; 4],
    /// The running trace's frame's base, in bytes past the stack's first
    /// value, written when a trace with side cells is entered: only their
    /// code reads it.
    pub(crate) base: TCellU32,
    /// The Vm's stack (its `LVec`, whose buffer pointer the code reads at
    /// [`crate::runtime::mem::LVec::PTR_OFFSET`]: the buffer moves when the
    /// stack grows), written with `base`: the Vm may have moved since.
    pub(crate) stack: TCellPtr,
    /// The running root trace by address (0: none).
    pub(crate) running: TCellPtr,
    /// The links taken in a row so far, each `[typed exit, running trace,
    /// linked trace]` (see `relink::enter`): `trail_len` of them at
    /// `trail_at`, which has room for `trail_cap`.
    pub(crate) trail_at: TCellPtr,
    pub(crate) trail_len: TCellU32,
    pub(crate) trail_cap: TCellU32,
    /// Where the innermost dispatch's links start in the trail.
    pub(crate) link_base: TCellU32,
    /// Non-zero while hot counting pauses after a collection exit, which
    /// entering a trace ends.
    pub(crate) quiet: TCellU32,
    /// Set by the optimizing tier's code that leaves a link to the Vm, for
    /// the Vm to count why (see `JitCounters::links_by_vm`).
    pub(crate) link_left: TCellU32,
    /// What `trail_at` points at: as long as the room, its length unused.
    trail: LVec<[u32; 3]>,
}

/// In [`RunWords::blocked`]: a collection check is due at the exit.
pub(crate) const BLOCKED_GC_EXIT: usize = 0;
/// In [`RunWords::blocked`]: an error is parked for the dispatcher.
pub(crate) const BLOCKED_PENDING_ERR: usize = 1;
/// In [`RunWords::blocked`]: the exit left something unmade.
pub(crate) const BLOCKED_UNMADE: usize = 2;

/// Where the code finds each field of a [`RunWords`], in bytes.
#[derive(Clone, Copy, Debug)]
pub struct RunWordsOffsets {
    /// `RunWords::blocked`'s (32 bits).
    pub blocked: i32,
    /// `RunWords::base`'s (32 bits).
    pub base: i32,
    /// `RunWords::stack`'s.
    pub stack: i32,
    /// `RunWords::running`'s.
    pub running: i32,
    /// `RunWords::trail_at`'s.
    pub trail_at: i32,
    /// `RunWords::trail_len`'s (32 bits).
    pub trail_len: i32,
    /// `RunWords::trail_cap`'s (32 bits).
    pub trail_cap: i32,
    /// `RunWords::link_base`'s (32 bits).
    pub link_base: i32,
    /// `RunWords::quiet`'s (32 bits).
    pub quiet: i32,
    /// `RunWords::link_left`'s (32 bits).
    pub link_left: i32,
}

impl RunWords {
    pub(crate) fn new(mem: MemRef) -> Self {
        Self {
            blocked: [const { TCellBool::new(false) }; 4],
            base: TCellU32::new(0),
            stack: TCellPtr::null(),
            running: TCellPtr::null(),
            trail_at: TCellPtr::null(),
            trail_len: TCellU32::new(0),
            trail_cap: TCellU32::new(0),
            link_base: TCellU32::new(0),
            quiet: TCellU32::new(0),
            link_left: TCellU32::new(0),
            trail: LVec::new(mem),
        }
    }

    /// The fields' offsets.
    pub const fn offsets() -> RunWordsOffsets {
        use std::mem::offset_of;
        RunWordsOffsets {
            blocked: offset_of!(RunWords, blocked) as i32,
            base: offset_of!(RunWords, base) as i32,
            stack: offset_of!(RunWords, stack) as i32,
            running: offset_of!(RunWords, running) as i32,
            trail_at: offset_of!(RunWords, trail_at) as i32,
            trail_len: offset_of!(RunWords, trail_len) as i32,
            trail_cap: offset_of!(RunWords, trail_cap) as i32,
            link_base: offset_of!(RunWords, link_base) as i32,
            quiet: offset_of!(RunWords, quiet) as i32,
            link_left: offset_of!(RunWords, link_left) as i32,
        }
    }

    /// Sets or clears byte `b` of [`RunWords::blocked`].
    #[inline(always)]
    pub(crate) fn block(&self, b: usize, on: bool) {
        self.blocked[b].set(on);
    }
}

impl super::JitState {
    /// The pc compiled code left a collection check at (see `gc_exit`).
    #[inline(always)]
    pub fn gc_exit(&self) -> Option<u32> {
        self.gc_exit
    }

    #[inline(always)]
    pub(crate) fn set_gc_exit(&mut self, pc: Option<u32>) {
        self.gc_exit = pc;
        self.words.block(BLOCKED_GC_EXIT, pc.is_some());
    }

    #[inline(always)]
    pub(crate) fn take_gc_exit(&mut self) -> Option<u32> {
        let pc = self.gc_exit.take();
        // the byte is down whenever no check is due
        if pc.is_some() {
            self.words.block(BLOCKED_GC_EXIT, false);
        }
        pc
    }

    /// The error a helper of compiled code parked (see `pending_err`).
    #[inline(always)]
    pub fn pending_err(&self) -> Option<&crate::vm::error::LuaError> {
        self.pending_err.as_ref()
    }

    #[inline(always)]
    pub(crate) fn set_pending_err(&mut self, e: Option<crate::vm::error::LuaError>) {
        self.words.block(BLOCKED_PENDING_ERR, e.is_some());
        self.pending_err = e;
    }

    /// Drops the parked error, if any: the byte is down whenever none is.
    #[inline(always)]
    pub(crate) fn clear_pending_err(&mut self) {
        if self.pending_err.is_some() {
            self.set_pending_err(None);
        }
    }

    /// Whether [`RunWords::blocked`] says what the flags it stands for
    /// say, `unmade` being whether the exit left something unmade.
    pub(crate) fn blocked_agrees(&self, unmade: bool) -> bool {
        let b = &self.words.blocked;
        b[BLOCKED_GC_EXIT].get() == self.gc_exit.is_some()
            && b[BLOCKED_PENDING_ERR].get() == self.pending_err.is_some()
            && b[BLOCKED_UNMADE].get() == unmade
            && !b[3].get()
    }
}
