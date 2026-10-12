//! A running native's record on the call stack.

use crate::runtime::{Gc, NativeClosure};

/// Where a running native sits: its value-stack window and the number of
/// frames below it when it was entered. 32 bytes: one is pushed on every
/// native call.
#[derive(Clone, Copy)]
pub(crate) struct NativeAct {
    pub(crate) nc: Gc<NativeClosure>,
    pub(crate) func_slot: u32,
    pub(crate) nargs: u32,
    /// the frames below it (low 24 bits) and the `__call` metamethods
    /// resolved to reach it (PUC 5.5 `CIST_CCMT`, the high byte)
    depth_ccmt: u32,
    /// what the C function has pushed (or dropped) since it was entered
    /// with its arguments, counted from the slot past them (see
    /// `Vm::native_push`); nothing is computed when it is pushed
    pub(crate) top_off: i32,
    /// the slot past what PUC's `CallInfo` of the C function reserves
    /// (`ci->top`): `LUA_MINSTACK` above its arguments, raised by the
    /// room `lua_checkstack` grants it. A protected call's catch and the
    /// collector shrink the stack to what the frames reserve.
    pub(crate) ci_top: u32,
}

const _: () = assert!(std::mem::size_of::<NativeAct>() <= 32);

impl NativeAct {
    /// `nc` entered at `func_slot` with `nargs` arguments, above `depth`
    /// frames, its top just past the arguments.
    pub(crate) fn new(
        nc: Gc<NativeClosure>,
        func_slot: u32,
        nargs: u32,
        depth: usize,
        ccmt: u8,
    ) -> Self {
        // the frame limit keeps the depth far below 2^24
        let depth_ccmt = depth as u32 | (u32::from(ccmt) << 24);
        NativeAct {
            nc,
            func_slot,
            nargs,
            depth_ccmt,
            top_off: 0,
            ci_top: func_slot + 1 + nargs + crate::vm::exec::stack_size::MIN_STACK,
        }
    }

    /// PUC's `L->top` while it runs: its arguments, then what the C
    /// function has pushed or dropped since.
    pub(crate) fn top(&self) -> u32 {
        (self.func_slot + 1 + self.nargs).wrapping_add_signed(self.top_off)
    }

    /// The frames below it when it was entered.
    pub(crate) fn depth(&self) -> u32 {
        self.depth_ccmt & 0x00ff_ffff
    }

    /// `__call` metamethods resolved to reach it.
    pub(crate) fn ccmt(&self) -> u8 {
        (self.depth_ccmt >> 24) as u8
    }
}
