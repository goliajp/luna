//! Compiled code handing its levels to the interpreter before an op that
//! would check for a collection with a step due, or that it cannot run
//! natively: each level records its frame (its base, the pc the
//! interpreter goes on from, its registers), deepest first, and returns;
//! the levels become interpreter frames, and the interpreter runs the op
//! with its own allocation and check, as PUC's interpreter runs it.

use super::*;

pub(crate) mod frames;
pub(crate) mod top;

#[cfg(test)]
mod tests;

/// What became of a call [`Vm::try_jit_call_op`] was asked to run.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::vm::exec) enum JitRun {
    /// not compiled code's: the interpreter makes the call
    NotRun,
    /// done, its results placed
    Done,
    /// handed to the interpreter part of the way: its levels are frames
    Handed,
}

impl Vm {
    /// Whether an op of compiled code that counts `bytes` and then checks
    /// for a collection would find a step due: then the interpreter runs
    /// it. A `gc-trace` build has the interpreter run every such op, so
    /// that its checks are traced.
    pub(crate) fn jit_due_after(&self, bytes: usize) -> bool {
        if crate::runtime::gc_hooks::TRACE {
            return true;
        }
        self.heap.gc_due_after(bytes)
    }

    /// Whether compiled code's `NewTable` of operands `b`, `c`, `k` would
    /// find a collection step due when it checks after making the table
    /// (PUC's size of it, as the operands give it): then the interpreter
    /// runs it. A table the operands cannot give is the interpreter's too.
    pub(crate) fn jit_table_due(&self, b: u32, c: u32, k: bool) -> bool {
        match crate::runtime::table::new_table_sizes(b, c, k) {
            Some((a, h)) => {
                let h = if h == 0 { 0 } else { h.next_power_of_two() };
                self.jit_due_after(self.heap.puc_table_bytes(a, h))
            }
            None => true,
        }
    }

    /// Record the frame of the compiled level at `base` (level 0's when
    /// `None`), which the interpreter goes on with at `pc`, its registers
    /// `vals`: onto the stack, allocating nothing (see [`frames`]).
    pub(crate) fn jit_handoff_frame(
        &mut self,
        base: Option<u32>,
        pc: u32,
        vals: impl ExactSizeIterator<Item = Value>,
    ) {
        let base = base.unwrap_or(self.jit.levels_slot);
        // the interpreter runs the op a helper parked a deopt for
        self.jit.set_pending_err(None);
        let m = &mut self.jit.handoff;
        let n = frames::write_level(&mut self.stack, m, base, vals);
        if base == self.jit.levels_slot {
            (m.pc0, m.n0) = (pc, n);
        } else {
            self.stack[base as usize - 1] = frames::record(pc, n);
        }
    }

    /// How many levels compiled code has handed over and the interpreter
    /// has not taken up yet.
    pub(crate) fn jit_handed_levels(&self) -> usize {
        self.jit.handoff.levels as usize
    }

    /// The levels compiled `cl`, called at `func_slot`, handed over become
    /// interpreter frames, level 0 returning `nresults` values; each level
    /// above it returns what its caller's call asks for. The levels were
    /// counted as calls already. A frame that cannot be pushed is the
    /// memory error of the call the level below it was making, as in the
    /// interpreter.
    pub(in crate::vm::exec) fn jit_resume_handoff(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nresults: i32,
        from_c: bool,
    ) -> Result<(), LuaError> {
        let r = self.jit_resume_levels(cl, func_slot, nresults, from_c);
        // what the levels wrote is the frames' now, or dead past the top
        self.jit.handoff = frames::HandoffMarks::default();
        r
    }

    /// The pc and register count of the level at `base`, `k` levels up.
    fn handed_level(&self, k: u32, base: u32) -> (u32, u32) {
        let m = &self.jit.handoff;
        if k == 0 {
            (m.pc0, m.n0)
        } else {
            frames::read_record(self.stack[base as usize - 1])
        }
    }

    fn jit_resume_levels(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nresults: i32,
        from_c: bool,
    ) -> Result<(), LuaError> {
        let code = &cl.proto.code;
        let max = u32::from(cl.proto.max_stack);
        let m = self.jit.handoff;
        // the deepest level's op runs first, with the top it reads
        let (mut base, mut pc) = (func_slot + 1, m.pc0);
        for k in 1..m.levels {
            base = frames::next_base(code, base, pc);
            pc = self.handed_level(k, base).0;
        }
        let top = base + top::handoff_top(code, pc, max);
        // an emergency collection keeps what the levels wrote (see
        // `Vm::alloc_top`)
        self.grow_stack(m.hi.max(top) as usize)?;
        let (mut base, mut want) = (func_slot + 1, nresults);
        for k in 0..m.levels {
            let (pc, n) = self.handed_level(k, base);
            debug_assert!(m.levels == 1 || n == max, "a level holds all its registers");
            let deepest = k + 1 == m.levels;
            let next = (!deepest).then(|| frames::next_base(code, base, pc));
            if let Err(e) = self.retry_oom(|vm| vm.frames.reserve(1)) {
                // the call the level below was making raises it
                self.top = base;
                return Err(e);
            }
            let fs = base - 1;
            self.stack[fs as usize] = Value::Closure(cl);
            // a self call the interpreter is to make calls the function
            // compiled code never kept in its register, unless a deeper
            // level's frame took the slot
            let op = code[pc as usize];
            let a = base + op.a();
            let free = next.is_none_or(|nb| a < nb - 1 || a >= m.hi);
            if matches!(
                op.op(),
                crate::vm::isa::Op::Call | crate::vm::isa::Op::TailCall
            ) && free
            {
                self.stack[a as usize] = Value::Closure(cl);
            }
            self.push_handed_frame(cl, base, pc, want, from_c && k == 0, k == 0);
            // the level above returns what this one's call asks for
            if let Some(call) = pc.checked_sub(1).map(|pc| code[pc as usize]) {
                want = call.c() as i32 - 1;
            }
            if let Some(nb) = next {
                base = nb;
            }
        }
        self.top = top;
        Ok(())
    }

    /// The frame of a level handed over at `base`, going on at `pc`;
    /// level 0's (`first`) is the call the interpreter was making.
    fn push_handed_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        pc: u32,
        nresults: i32,
        from_c: bool,
        first: bool,
    ) {
        let cf = CallFrame::Lua(Frame {
            closure: cl,
            base,
            pc,
            func_slot: base - 1,
            nresults,
            hook_oldpc: u32::MAX,
            from_c,
            n_varargs: 0,
            tm: if first { self.pending_tm.take() } else { None },
            is_hook: first && std::mem::take(&mut self.pending_is_hook),
            tailcalls: if first {
                std::mem::take(&mut self.pending_tailcalls)
            } else {
                0
            },
            ccmt: if first {
                std::mem::take(&mut self.pending_ccmt)
            } else {
                0
            },
        });
        frames_push_sync(&mut self.frames, &mut self.frames_top, &mut self.trap, cf);
    }
}
