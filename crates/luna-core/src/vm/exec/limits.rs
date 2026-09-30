//! What the loop head does when the instruction budget or the memory cap
//! runs out.

use super::*;

impl Vm {
    #[cold]
    #[inline(never)]
    pub(super) fn instr_budget_exhausted(&mut self) -> LuaError {
        self.instr_budget = None;
        // Async-mode cooperative
        // yield. Set a sentinel flag so `exec_with`
        // propagates the Err without `unwind` running
        // (mirroring the `yielding.is_some()` path),
        // and `call_value_impl` preserves the call
        // frames for the next `poll`. Translation back
        // to `DispatchOutcome::BudgetExhausted` happens
        // in `drive_one`. The Err value itself is
        // `Value::Nil` — a pure sentinel, never seen by
        // user code.
        if self.async_mode {
            self.host_yield_pending = true;
            return LuaError(Value::Nil);
        }
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
        // disarm + raise if the cap is still breached after
        // collection. PUC's `LUA_GCEMERGENCY` path matches.
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
        // caller resumes). For fire-once cap path this
        // residual over-root is acceptable; there is no
        // full per-frame walk because a strong/weak pass
        // split is semantically impossible — the weak pass
        // depends on strong-pass marks.
        let cap_root_top = self
            .frames
            .iter()
            .rev()
            .find_map(CallFrame::lua)
            .map(|f| f.base + f.closure.proto.max_stack as u32)
            .unwrap_or(self.top);
        self.gc_top = cap_root_top.max(self.top);
        self.collect_garbage();
        if self.heap.bytes() > cap {
            self.heap.mem_cap = None;
            let s = Value::Str(self.heap.intern(b"memory cap exceeded"));
            return Err(LuaError(s));
        }
        Ok(())
    }
}
