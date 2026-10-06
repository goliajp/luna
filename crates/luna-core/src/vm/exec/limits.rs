//! What the loop head does when the instruction budget or the memory cap
//! runs out.
//!
//! Both limits are sandbox boundaries, so running out is not an error the
//! script can catch and recover from: the limit stays at zero until the
//! host arms a new one, and at zero the loop head raises the same error
//! before every instruction, whoever is running it (a `pcall` caller, an
//! `xpcall` handler, a `__close` or `__gc` handler, a metamethod, a
//! library callback, a coroutine). Zero is the exhausted state itself,
//! so the normal path tests nothing new: a budget of zero runs out on
//! its next tick, and a cap of zero is exceeded by any heap.

use super::*;

impl Vm {
    /// An instruction budget or a memory cap is armed (exhausted or not):
    /// no compiled code is entered, since it checks neither. Recomputed by
    /// whatever changes either limit.
    #[inline]
    pub(crate) fn sync_limited(&mut self) {
        self.limited = self.instr_budget.is_some() || self.heap.mem_cap.is_some();
    }

    #[cold]
    #[inline(never)]
    pub(super) fn instr_budget_exhausted(&mut self) -> LuaError {
        // Async-mode cooperative
        // yield. Set a sentinel flag so `exec_with`
        // propagates the Err without `unwind` running
        // (mirroring the `yielding.is_some()` path),
        // and `call_value_impl` preserves the call
        // frames for the next `poll`. Translation back
        // to `DispatchOutcome::BudgetExhausted` happens
        // in `drive_one`, which arms the next slice. The Err value
        // itself is `Value::Nil` — a pure sentinel, never seen by
        // user code.
        if self.async_mode {
            self.instr_budget = None;
            self.sync_limited();
            self.host_yield_pending = true;
            return LuaError(Value::Nil);
        }
        self.instr_budget = Some(0);
        // Classify the trip so embedders can
        // distinguish budget exhaustion from a
        // generic Runtime error and retry / give up
        // accordingly.
        self.last_error_kind = crate::vm::error::LuaErrorKind::InstrBudget;
        let s = Value::Str(self.heap.intern(b"instruction budget exceeded"));
        LuaError(s)
    }

    #[cold]
    #[inline(never)]
    pub(super) fn mem_cap_exceeded(&mut self, cap: usize) -> Result<(), LuaError> {
        // First try a full collect — embedders set tight caps
        // and the overshoot may be reclaimable (closures kept
        // by short-lived frames, intermediate strings). Only
        // raise if the cap is still breached after
        // collection. PUC's `LUA_GCEMERGENCY` path matches.
        // A cap of zero is the exceeded state: nothing to reclaim.
        //
        // Root up to the deepest Lua frame's
        // `base + max_stack` window rather than the entire
        // `self.stack.len()`
        // (covers register operands the current opcode
        // might reference). The cap fires during table
        // mutation in a tight `a[i] = i` loop where `a`
        // lives at a frame-register slot past `self.top`
        // (OP_NEWINDEX doesn't advance top); the deepest
        // frame's max_stack window provably covers it
        // since `a` is a register of the executing proto.
        //
        // Still over-roots caller frames' dead regs
        // (slots between caller.base and the callee
        // func_slot are live; slots past callee
        // func_slot in caller's frame are dead until
        // caller resumes). For the cap path this
        // residual over-root is acceptable; there is no
        // full per-frame walk because a strong/weak pass
        // split is semantically impossible — the weak pass
        // depends on strong-pass marks.
        if cap != 0 {
            let cap_root_top = self
                .frames
                .iter()
                .rev()
                .find_map(CallFrame::lua)
                .map(|f| f.base + f.closure.proto.max_stack as u32)
                .unwrap_or(self.top);
            self.gc_top = cap_root_top.max(self.top);
            self.collect_garbage();
            if self.heap.bytes() <= cap {
                return Ok(());
            }
            self.heap.mem_cap = Some(0);
        }
        self.last_error_kind = crate::vm::error::LuaErrorKind::MemoryCap;
        let s = Value::Str(self.heap.intern(b"memory cap exceeded"));
        Err(LuaError(s))
    }
}
