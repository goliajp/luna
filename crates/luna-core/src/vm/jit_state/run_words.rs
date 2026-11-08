//! The words of a Vm that compiled code reads where it would otherwise call
//! into the Vm: whether the exit it is at left something to the dispatcher,
//! and where the running trace's frame is on the stack.
//!
//! Each Vm has one, boxed so that its address holds while the Vm moves; the
//! code finds it through a relocation each Vm writes its own address into.
//! The Vm writes the words on one thread at a time, between the code's
//! reads, as for the side cells.

use crate::jit::send_compat::{TCellPtr, TCellU32};

/// See the module comment. The code reads the fields at the offsets
/// [`RunWords::offsets`] gives.
#[repr(C)]
pub struct RunWords {
    /// Non-zero while an exit left something for the dispatcher to do: a
    /// collection check ([`BLOCKED_GC_EXIT`]), an error to raise
    /// ([`BLOCKED_PENDING_ERR`]), something to make ([`BLOCKED_UNMADE`]).
    pub(crate) blocked: TCellU32,
    /// The running trace's frame's base, in bytes past the stack's first
    /// value.
    pub(crate) base: TCellU32,
    /// The Vm's stack (its `LVec`, whose buffer pointer the code reads at
    /// [`crate::runtime::mem::LVec::PTR_OFFSET`]), written each time a
    /// trace is entered: the buffer moves when the stack grows.
    pub(crate) stack: TCellPtr,
}

/// In [`RunWords::blocked`]: a collection check is due at the exit.
pub(crate) const BLOCKED_GC_EXIT: u32 = 1;
/// In [`RunWords::blocked`]: an error is parked for the dispatcher.
pub(crate) const BLOCKED_PENDING_ERR: u32 = 2;
/// In [`RunWords::blocked`]: the exit left something unmade.
pub(crate) const BLOCKED_UNMADE: u32 = 4;

/// Where the code finds each field of a [`RunWords`], in bytes.
#[derive(Clone, Copy, Debug)]
pub struct RunWordsOffsets {
    /// `RunWords::blocked`'s (32 bits).
    pub blocked: i32,
    /// `RunWords::base`'s (32 bits).
    pub base: i32,
    /// `RunWords::stack`'s.
    pub stack: i32,
}

impl RunWords {
    pub(crate) fn new() -> Self {
        Self {
            blocked: TCellU32::new(0),
            base: TCellU32::new(0),
            stack: TCellPtr::null(),
        }
    }

    /// The fields' offsets.
    pub const fn offsets() -> RunWordsOffsets {
        RunWordsOffsets {
            blocked: std::mem::offset_of!(RunWords, blocked) as i32,
            base: std::mem::offset_of!(RunWords, base) as i32,
            stack: std::mem::offset_of!(RunWords, stack) as i32,
        }
    }

    /// Sets or clears `bit` of [`RunWords::blocked`].
    #[inline(always)]
    pub(crate) fn block(&self, bit: u32, on: bool) {
        let b = self.blocked.get();
        self.blocked.set(if on { b | bit } else { b & !bit });
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
        self.words.block(BLOCKED_GC_EXIT, false);
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

    /// Whether [`RunWords::blocked`] says what the flags it stands for
    /// say, `unmade` being whether the exit left something unmade.
    pub(crate) fn blocked_agrees(&self, unmade: bool) -> bool {
        let bit = |on: bool, b: u32| if on { b } else { 0 };
        let want = bit(self.gc_exit.is_some(), BLOCKED_GC_EXIT)
            | bit(self.pending_err.is_some(), BLOCKED_PENDING_ERR)
            | bit(unmade, BLOCKED_UNMADE);
        self.words.blocked.get() == want
    }
}
