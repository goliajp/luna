//! The continuation records: host, close and metamethod continuations.

/// Where a [`ContKind::Host`](super::ContKind::Host) continuation's values land, and which of the
/// C API's records describes it.
#[derive(Clone, Copy)]
pub struct HostCont {
    /// first stack slot of the called function's results, or of the values
    /// a resume passes
    pub results_at: u32,
    /// the C API's own index for the continuation
    pub token: u32,
}

/// Per-iteration state for a chain of `__close` handlers driven through the
/// interpreter loop. When a handler is pushed onto the call stack, this rides
/// in a `Cont::Close` frame underneath it so a `coroutine.yield` from the
/// handler preserves the close iteration with the rest of the thread.
#[derive(Clone, Copy)]
pub struct CloseCont {
    /// the close threshold: keep closing tbc slots ≥ from until exhausted
    pub from: u32,
    /// an error object is threaded through the remaining handlers; it sits
    /// in the continuation's own stack slot (`NativeCont::func_slot`), just
    /// below the handler's call, where the collector sees it
    pub has_pending: bool,
    /// what to do once every slot ≥ from is closed
    pub after: AfterClose,
}

/// What to run once `begin_close` has drained every tbc slot.
#[derive(Clone, Copy)]
pub enum AfterClose {
    /// `OP_Close` (block-end close): nothing else; next instruction continues.
    Block,
    /// `OP_Return*`: pop the Lua frame whose `OP_Return` triggered the close
    /// and deliver `nret` results from `[abs_a, abs_a + nret)` to the frame's
    /// `func_slot`. `from_native` mirrors the original op's hook flag.
    Return {
        /// Absolute stack index of the first return value.
        abs_a: u32,
        /// Number of return values.
        nret: u32,
        /// Mirrors the original op's hook-fired flag.
        from_native: bool,
    },
    /// Error unwind: the close runs while unwinding a Lua frame. When every
    /// handler is done, pop the deferred Lua frame, truncate to `func_slot`,
    /// and re-raise the threaded error: the original one, or the last a
    /// handler raised (PUC luaF_close).
    ResumeUnwind {
        /// Slot to truncate the value stack to before re-raising.
        func_slot: u32,
    },
}

/// How to complete a VM instruction once its metamethod returns.
#[derive(Clone, Copy)]
pub struct MetaCont {
    /// What to do with the metamethod's return value.
    pub action: MetaAction,
    /// the interrupted frame's `top` to restore after the metamethod returns
    pub saved_top: u32,
    /// the stack's length before the call, to give back after it: the call
    /// sits at the frame's window end, which can be below an outer window
    pub saved_len: u32,
}

/// Per-op finishing action for a yielded metamethod call.
#[derive(Clone, Copy)]
pub enum MetaAction {
    /// arithmetic / index / unary / length: store the single result at `dst`
    Store {
        /// Destination register receiving the metamethod's first result.
        dst: u32,
    },
    /// `__newindex`: the metamethod has no result to keep
    Discard,
    /// comparison (`__eq`/`__lt`/`__le`): the truthiness of the result feeds the
    /// conditional skip — the following JMP runs iff `result.truthy() == k`.
    /// `negate=true` flips the truthiness first, for the ≤5.3 `__le` →
    /// `not __lt(b, a)` synthesis path where the metamethod is `__lt` but
    /// the operator was `<=`.
    Compare {
        /// Sense of the conditional skip the comparison op was emitted for.
        k: bool,
        /// True when the 5.3 `__le → not __lt(b,a)` synthesis is in effect.
        negate: bool,
    },
    /// `__concat`: store the result at `dst`, set `top = dst + 1`, then continue
    /// folding the operands still at `[base_a .. top)` (PUC finishOp re-runs).
    Concat {
        /// Destination register for the metamethod's result.
        dst: u32,
        /// First operand register of the original concat span.
        base_a: u32,
    },
}
