//! Call-stack records: Lua frames and the continuations native calls,
//! `__close` handlers and metamethods leave on the stack.

use crate::runtime::function::LuaClosure;
use crate::runtime::heap::Gc;
use crate::runtime::value::Value;

mod conts;
pub use conts::*;

/// An activation record on a thread's call stack. Pure data (closure handle +
/// stack offsets), so it lives in `runtime` where the GC can trace a suspended
/// coroutine's frames.
#[derive(Clone, Copy)]
pub struct Frame {
    /// Currently executing closure.
    pub closure: Gc<LuaClosure>,
    /// stack index of register 0
    pub base: u32,
    /// Program counter (index into `closure.proto.code`).
    pub pc: u32,
    /// stack slot of the function (results land here)
    pub func_slot: u32,
    /// number of extra (vararg) arguments, living on the stack just below `base`
    /// at `func_slot+1 .. func_slot+1+n_varargs` (PUC `CallInfo.u.l.nextraargs`).
    /// `OP_VARARG`/`OP_VARGIDX` read them there; a named vararg only materializes
    /// a heap table when it is written / escapes / is `_ENV`.
    pub n_varargs: u32,
    /// results expected by the caller (-1 = all)
    pub nresults: i32,
    /// pc the line hook last observed in this frame (PUC CallInfo `oldpc`);
    /// `u32::MAX` on a fresh frame so its first instruction fires a line event
    pub hook_oldpc: u32,
    /// true if this Lua frame was entered across a C boundary (call_value: a
    /// metamethod, pcall, __close handler, or a coroutine body). The debug
    /// interface does not read it: it places the running natives themselves
    /// among the frames.
    pub from_c: bool,
    /// the metamethod event this frame is handling; the debug interface
    /// reads [`FrameTm::Gc`] to recognize a finalizer (PUC `CIST_FIN`), and
    /// names other handlers from the instruction that called them.
    pub tm: Option<FrameTm>,
    /// true when this frame is the hook function itself (PUC sets
    /// `CIST_HOOKED`). `debug.getinfo(1).namewhat` returns `"hook"` for it.
    pub is_hook: bool,
    /// PUC `ci->u.l.tailcalls` — how many tail calls have collapsed into
    /// this activation slot. Each `OP_TailCall` chain adds one. 5.1
    /// `lua_getstack` reports a synthetic `CIST_TAIL` level per count
    /// (so a deeply tail-recursive function shows `tailcalls` extra
    /// levels between itself and its real caller — 5.1 db.lua :372 walks
    /// `getinfo(2..lim)` and expects each to be `"tail"`). The 5.2+
    /// `istailcall` boolean is `tailcalls > 0`.
    pub tailcalls: u32,
    /// How many `__call` metamethods were resolved to reach this function;
    /// each adds one argument (PUC 5.5 `CIST_CCMT`, reported as
    /// `getinfo("t").extraargs`).
    pub ccmt: u8,
}

/// Which kind of metamethod a Lua frame was called as (PUC
/// `CallInfo.u.l.tm`, reduced to what the VM asks of it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum FrameTm {
    /// a `__gc` finalizer
    Gc,
    /// a `__close` handler
    Close,
    /// any other metamethod
    Meta,
}

/// An entry on a thread's call stack: either a Lua activation record or a
/// continuation frame standing in for a *yieldable native* (pcall/xpcall).
///
/// A `Cont` sits just below the call it protects. When that call returns,
/// yields-to-completion, or errors, the interpreter consumes the `Cont` to wrap
/// the outcome — the analogue of PUC `lua_pcallk`'s continuation `k`. Keeping it
/// on the same stack as Lua frames means a `coroutine.yield` crossing it is
/// preserved and restored automatically with the thread's saved context.
#[derive(Clone, Copy)]
pub enum CallFrame {
    /// A Lua activation record.
    Lua(
        /// The activation record.
        Frame,
    ),
    /// A continuation guarding a yieldable native call (pcall / xpcall /
    /// metamethod / `__close` / `__pairs`).
    Cont(
        /// The continuation record.
        NativeCont,
    ),
}

impl CallFrame {
    /// Borrow the inner Lua frame if this is a `Lua` variant.
    #[inline]
    pub fn lua(&self) -> Option<&Frame> {
        match self {
            CallFrame::Lua(f) => Some(f),
            CallFrame::Cont(_) => None,
        }
    }

    /// Mutably borrow the inner Lua frame if this is a `Lua` variant.
    #[inline]
    pub fn lua_mut(&mut self) -> Option<&mut Frame> {
        match self {
            CallFrame::Lua(f) => Some(f),
            CallFrame::Cont(_) => None,
        }
    }
}

/// A continuation frame for `pcall`/`xpcall`: where its wrapped result lands and
/// how to wrap it. Lives on the call stack below the protected call (see
/// [`CallFrame`]).
#[derive(Clone, Copy)]
pub struct NativeCont {
    /// What kind of protection this continuation represents.
    pub kind: ContKind,
    /// the protecting native's own stack slot — the wrapped status + values
    /// (`true, …` / `false, msg`) land here
    pub func_slot: u32,
    /// results the caller of pcall/xpcall expects (-1 = all)
    pub nresults: i32,
}

impl ContKind {
    /// Whether a protected call's continuation took a C level of PUC's.
    pub fn is_level(&self) -> bool {
        match self {
            ContKind::Pcall { level, .. } | ContKind::Xpcall { level, .. } => *level,
            _ => true,
        }
    }

    /// The slot a protected call's callee was called at, above the slot
    /// after the continuation (`Vm::callee_shift`).
    pub fn callee_shift(&self) -> i32 {
        match self {
            ContKind::Pcall { shift, .. } | ContKind::Xpcall { shift, .. } => i32::from(*shift),
            _ => 0,
        }
    }
}

/// Continuation kind for yieldable native dispatch.
#[derive(Clone, Copy)]
pub enum ContKind {
    /// `pcall(f, ...)` — wraps the result as `(true, ...)` / `(false, msg)`.
    Pcall {
        /// The call is a C level of PUC's (`Vm::begin_pcall`).
        level: bool,
        /// Where the callee was called, relative to the slot above the
        /// continuation (`Vm::callee_shift`).
        shift: i8,
    },
    /// xpcall: the message handler to run if the protected call errors
    Xpcall {
        /// Message handler function invoked on error.
        handler: Value,
        /// The call is a C level of PUC's (`Vm::begin_pcall`).
        level: bool,
        /// Where the callee was called (`Vm::callee_shift`).
        shift: i8,
    },
    /// a yieldable metamethod call triggered by a VM instruction (PUC's
    /// `luaV_finishOp`): on the metamethod's return the interrupted instruction
    /// is completed per `MetaCont`. A `coroutine.yield` inside the metamethod is
    /// preserved on the thread's frame stack like any other call.
    Meta(
        /// Continuation describing how to finish the interrupted op.
        MetaCont,
    ),
    /// a yieldable `__pairs` call from `pairs()` (PUC `lua_callk`); its (≤4,
    /// nil-padded) results are `pairs`'s own, and a yield is kept as pcall's
    Pairs {
        /// where it was called, counted from `pairs`'s slot
        at: u32,
    },
    /// a yieldable `__close` handler call driven by `begin_close` (PUC's
    /// `luaF_close` + `lua_callk` continuation). On the handler's return or
    /// error, the close iteration resumes from `CloseCont`'s state and either
    /// invokes the next handler (pushing a fresh Cont::Close) or executes the
    /// recorded `AfterClose` action.
    Close(
        /// Per-iteration close state.
        CloseCont,
    ),
    /// a C function's continuation (PUC `lua_callk`, `lua_pcallk`,
    /// `lua_yieldk`): the C function sits at the frame's `func_slot` and has
    /// been left by a yield. When the call it made returns, or the coroutine
    /// it yielded is resumed, the C API runs the continuation with the values
    /// from `results_at` on, and what the continuation returns are the C
    /// function's results.
    Host(HostCont),
}

#[cfg(test)]
mod tests {
    use super::*;

    // a frame is written on every call and read on every return, and a
    // continuation takes no more room on the call stack than a Lua frame
    #[test]
    fn a_call_stack_entry_is_at_most_40_bytes() {
        assert!(std::mem::size_of::<Frame>() <= 40);
        assert!(std::mem::size_of::<NativeCont>() <= 32);
        assert!(std::mem::size_of::<CallFrame>() <= 40);
    }
}
