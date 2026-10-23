//! Compiled code handing its levels to the interpreter before an op that
//! would check for a collection with a step due, or that it cannot run
//! natively: each level records its frame (its base, the pc the
//! interpreter goes on from, its registers), deepest first, and returns;
//! the levels become interpreter frames, and the interpreter runs the op
//! with its own allocation and check, as PUC's interpreter runs it.

use super::*;

/// What became of a call [`Vm::try_jit_call_op`] was asked to run.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum JitRun {
    /// not compiled code's: the interpreter makes the call
    NotRun,
    /// done, its results placed
    Done,
    /// handed to the interpreter part of the way: its levels are frames
    Handed,
}

/// A compiled level's frame, as the interpreter goes on with it.
pub(crate) struct HandoffFrame {
    base: u32,
    pc: u32,
    vals: Vec<Value>,
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
    /// `vals`.
    pub(crate) fn jit_handoff_frame(&mut self, base: Option<u32>, pc: u32, vals: Vec<Value>) {
        let base = base.unwrap_or(self.jit.levels_slot);
        // the interpreter runs the op a helper parked a deopt for
        self.jit.pending_err = None;
        self.jit.handoff.push(HandoffFrame { base, pc, vals });
    }

    /// The levels compiled `cl`, called at `func_slot`, handed over become
    /// interpreter frames, level 0 returning `nresults` values; each level
    /// above it returns what its caller's call asks for. The levels were
    /// counted as calls already.
    pub(super) fn jit_resume_handoff(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nresults: i32,
        from_c: bool,
    ) -> Result<(), LuaError> {
        let levels = std::mem::take(&mut self.jit.handoff);
        let max = u32::from(cl.proto.max_stack);
        let top = levels.first().map_or(func_slot + 1 + max, |f| f.base + max);
        let end = levels
            .iter()
            .map(|f| f.base + f.vals.len() as u32)
            .max()
            .unwrap_or(0)
            .max(top);
        // the levels' values are held nowhere else until they are on the
        // stack: rooted across what may collect in an emergency
        let at = self.lib_roots.mark();
        for f in &levels {
            self.lib_roots.extend_from_slice(&f.vals);
        }
        let room = self
            .grow_stack(end as usize)
            .and_then(|()| self.retry_oom(|vm| vm.frames.reserve(levels.len())));
        self.lib_roots.release(at);
        room?;
        let mut want = nresults;
        for (k, f) in levels.iter().rev().enumerate() {
            let fs = f.base - 1;
            self.stack[fs as usize] = Value::Closure(cl);
            self.stack[f.base as usize..f.base as usize + f.vals.len()].copy_from_slice(&f.vals);
            // a self call the interpreter is to make calls the function
            // compiled code never kept in its register
            let op = cl.proto.code[f.pc as usize];
            if matches!(
                op.op(),
                crate::vm::isa::Op::Call | crate::vm::isa::Op::TailCall
            ) {
                self.stack[(f.base + op.a()) as usize] = Value::Closure(cl);
            }
            let cf = CallFrame::Lua(Frame {
                closure: cl,
                base: f.base,
                pc: f.pc,
                func_slot: fs,
                nresults: want,
                hook_oldpc: u32::MAX,
                from_c: from_c && k == 0,
                n_varargs: 0,
                // level 0 is the call the interpreter was making
                tm: if k == 0 { self.pending_tm.take() } else { None },
                is_hook: k == 0 && std::mem::take(&mut self.pending_is_hook),
                tailcalls: if k == 0 {
                    std::mem::take(&mut self.pending_tailcalls)
                } else {
                    0
                },
                ccmt: if k == 0 {
                    std::mem::take(&mut self.pending_ccmt)
                } else {
                    0
                },
            });
            frames_push_sync(&mut self.frames, &mut self.frames_top, &mut self.trap, cf);
            // the level above returns what this one's call asks for
            if let Some(call) = f.pc.checked_sub(1).map(|pc| cl.proto.code[pc as usize]) {
                want = call.c() as i32 - 1;
            }
        }
        self.top = top;
        Ok(())
    }
}
