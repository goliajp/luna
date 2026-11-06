//! The interpreter loop head: trap handling, continuation frames, trace
//! dispatch, then the fast loop and the opcodes it hands back.

use super::*;
mod finish_cont;

impl Vm {
    /// The loop head's slow path, taken while [`Vm::trap`] is set: tick the
    /// instruction budget, enforce the memory cap, then clear `trap` unless
    /// one of them, an armed hook or a continuation frame on top still needs
    /// the next pass through the loop head.
    #[inline]
    pub(super) fn trap_step(&mut self) -> Result<(), LuaError> {
        if let Some(b) = self.instr_budget.as_mut() {
            *b -= 1;
            if *b <= 0 {
                return Err(self.instr_budget_exhausted());
            }
        }
        if let Some(cap) = self.heap.mem_cap()
            && self.heap.cap_bytes() > cap
        {
            self.mem_cap_exceeded(cap)?;
        }
        self.trap = self.instr_budget.is_some()
            || self.heap.mem_cap().is_some()
            || self.hook_armed()
            || matches!(self.frames.last(), Some(CallFrame::Cont(_)));
        Ok(())
    }

    pub(super) fn run(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        // the host may have set a budget, a cap or a hook since the last run
        self.trap = true;
        let pre53 = self.version() <= LuaVersion::Lua53;
        let dbl = self.version() <= LuaVersion::Lua52;
        loop {
            if self.trap {
                self.trap_step()?;
            }
            // A continuation frame on top means the call it protected just
            // delivered its results. Pushing a continuation, or popping down
            // to one, sets `trap` (`frames_push_sync` / `frames_pop_sync`), so
            // the loop head looks for one only under `trap`.
            if self.trap
                && let Some(&CallFrame::Cont(nc)) = self.frames.last()
            {
                if let Some(vals) = self.finish_cont(nc, entry_depth)? {
                    return Ok(vals);
                }
                continue;
            }
            debug_assert!(
                matches!(self.frames.last(), Some(CallFrame::Lua(_))),
                "a continuation frame on top with `trap` clear"
            );
            let (cl, base, pc, oldpc, inst) = self.running_inst();
            // read once: with the trace JIT off this is the only JIT test an
            // instruction makes
            let trace_on = self.jit.trace_enabled;
            if self.trace_at(cl, pc, inst, base, trace_on)? {
                continue;
            }
            let fr = self.enter_inst(cl, pc, oldpc)?;
            let (fx, watch) = self.fast_entry(cl, fr, trace_on, entry_depth);
            let out = self.run_fast_for(fx, inst, pc + 1, (watch, trace_on, dbl, pre53))?;
            let inst = match out {
                fast::FastExit::Reload => continue,
                fast::FastExit::Slow(inst) => inst,
            };
            if let Some(vals) = self.run_slow(inst, entry_depth)? {
                return Ok(vals);
            }
            // the loop head reloads everything from the frame
        }
    }

    /// The running Lua frame's closure, base, pc and hook pc, and the
    /// instruction at its pc. A continuation frame on top was consumed.
    #[cfg_attr(luna_hot, inline(always))]
    fn running_inst(&self) -> (Gc<LuaClosure>, u32, u32, u32, Inst) {
        // GC runs only at the allocation safe points below (PUC's
        // `luaC_checkGC` sites), each with a precise `gc_top`; the loop head
        // no longer collects, so a stale full-window `gc_top` cannot leak in.
        //
        // SAFETY: the running thread always has a frame here, and it is a
        // Lua frame: the loop head consumed a continuation on top
        let f = match unsafe { self.frames.last().unwrap_unchecked() } {
            CallFrame::Lua(f) => f,
            // SAFETY: see above
            CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
        };
        let cl = f.closure;
        let base = f.base;
        let pc = f.pc;
        let oldpc = f.hook_oldpc;

        // SAFETY: `pc` is bounded by the compiler against `proto.code.len()`
        // — every branch / call op only sets `pc` to a valid index, and
        // function entry initialises pc=0 with a non-empty body. PUC's
        // `vmfetch` uses the equivalent unchecked load.
        let inst = unsafe { *cl.proto.code.get_unchecked(pc as usize) };
        (cl, base, pc, oldpc, inst)
    }

    /// The trace JIT's part of the instruction `inst` at `pc` of `cl` (frame
    /// `base`): a recording takes it, and a trace cached for `pc` runs.
    /// `Ok(true)` when a trace ran and the loop goes on where it left.
    #[cfg_attr(luna_hot, inline(always))]
    fn trace_at(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        inst: Inst,
        base: u32,
        trace_on: bool,
    ) -> Result<bool, LuaError> {
        // a recording takes the op (closing at its head, or dropped past its
        // length limit)
        if trace_on && self.jit.active_trace.is_some() {
            self.trace_record_step(cl, pc, inst, base);
        }
        // A trace cached for `pc` runs on the frame's registers, and the
        // interpreter goes on at its exit; one that comes back with
        // `pending_err` parked is a deopt, and the interpreter runs from `pc`
        // (the trace stays cached). Compiled code ticks no instruction budget
        // and checks no memory cap: while one is armed, every loop stays in the
        // interpreter. After a down-recursion trace's deopt the interpreter runs
        // its head op once (`suppress_downrec_admit_once`, read and cleared
        // here), or the trace would be admitted again forever.
        let admit = trace_on && cl.proto.has_dispatchable_trace.get() && !self.limited;
        let downrec_admit_blocked =
            admit && std::mem::take(&mut self.jit.suppress_downrec_admit_once);
        if admit
            && Vm::trace_head_at(&cl.proto, pc, downrec_admit_blocked)
            && self.trace_dispatch(cl, pc, base, downrec_admit_blocked)
        {
            // the check the trace's exit made raised
            if let Some(e) = self.jit.pending_raise.take() {
                return Err(e);
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Moves the running frame past the instruction at `pc` of `cl` and
    /// runs the count and line hooks for it: the frame, which a hook may
    /// have moved.
    #[cfg_attr(luna_hot, inline(always))]
    fn enter_inst(
        &mut self,
        cl: Gc<LuaClosure>,
        pc: u32,
        oldpc: u32,
    ) -> Result<*mut Frame, LuaError> {
        // PUC `vmfetch` increments savedpc BEFORE firing traceexec, so
        // hook code that consults `currentpc = savedpc - 1` lands on the
        // instruction now executing. luna mirrors that by advancing
        // `f.pc` to `pc + 1` before the hook block — local_at /
        // getinfo / line attribution all read f.pc, and the existing
        // `pc - 1` convention in those helpers then yields the current
        // instruction's pc (db.lua :696: local `A` visible at the
        // chunk's return line once OP_CLOSURE has advanced pc).
        //
        // Inline `top_frame_mut` for the hot path: top is guaranteed Lua
        // (cont frames drained above) so the and_then/Option layers are
        // dead weight.
        // SAFETY: the running thread has a frame while it executes
        // instructions
        let mut fr: *mut Frame = match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(fmut) => {
                fmut.pc = pc + 1;
                fmut
            }
            _ => unreachable!("Cont frame at pc bump"),
        };

        // count + line hooks (PUC traceexec): before executing the
        // instruction. Skipped while the hook itself runs. `trap` is set
        // whenever a hook is armed, so the common case tests one byte.
        if self.trap && self.hook_armed() {
            self.exec_hooks(cl, pc, oldpc)?;
            // a hook runs Lua code, which can move `self.frames`
            // SAFETY: as above
            fr = match unsafe { self.frames.last_mut().unwrap_unchecked() } {
                CallFrame::Lua(fmut) => fmut,
                _ => unreachable!("Cont frame after a hook"),
            };
        }
        Ok(fr)
    }

    /// The fast loop's state for frame `fr` of `cl`, and whether it stops at
    /// trace heads.
    #[cfg_attr(luna_hot, inline(always))]
    fn fast_entry(
        &self,
        cl: Gc<LuaClosure>,
        fr: *mut Frame,
        trace_on: bool,
        entry_depth: usize,
    ) -> (fast::Fast, bool) {
        let mut heads =
            [crate::runtime::function::TRACE_HEADS_NONE; crate::runtime::function::TRACE_HEADS_CAP];
        let stay = !self.trap
            && (!trace_on
                || self.jit.active_trace.is_none() && {
                    heads = cl.proto.trace_heads.get();
                    heads[0] != crate::runtime::function::TRACE_HEADS_MANY
                });
        let fx = fast::Fast {
            fr,
            trace_on,
            entry_depth,
            stay,
            heads,
        };
        let watch = !stay || heads[0] != crate::runtime::function::TRACE_HEADS_NONE;
        (fx, watch)
    }

    /// The fast loop instantiated for the loop's `(watch, trace_on, dbl,
    /// pre53)`: 5.1/5.2 (`dbl`) are pre-5.3 too. One call site per
    /// instance when optimised; one call through a pointer otherwise, which
    /// keeps the loop head's frame (on every level of a Lua call made from
    /// a native) from holding the arguments of eight calls.
    #[cfg_attr(luna_hot, inline(always))]
    fn run_fast_for(
        &mut self,
        fx: fast::Fast,
        inst: Inst,
        next: u32,
        modes: (bool, bool, bool, bool),
    ) -> Result<fast::FastExit, LuaError> {
        #[cfg(luna_hot)]
        macro_rules! inst {
            ($($m:literal),*) => {
                self.run_fast::<$($m),*>(fx, inst, next)
            };
        }
        #[cfg(not(luna_hot))]
        macro_rules! inst {
            ($($m:literal),*) => {
                Vm::run_fast::<$($m),*> as fn(&mut Vm, fast::Fast, Inst, u32) -> _
            };
        }
        #[allow(clippy::let_and_return)]
        let out = match modes {
            (true, _, true, _) => inst!(true, true, true, true),
            (false, true, true, _) => inst!(false, true, true, true),
            (false, false, true, _) => inst!(false, false, true, true),
            (true, _, false, true) => inst!(true, true, false, true),
            (false, true, false, true) => inst!(false, true, false, true),
            (false, false, false, true) => inst!(false, false, false, true),
            (true, _, false, false) => inst!(true, true, false, false),
            (false, true, false, false) => inst!(false, true, false, false),
            (false, false, false, false) => inst!(false, false, false, false),
        };
        #[cfg(not(luna_hot))]
        let out = out(self, fx, inst, next);
        out
    }
}
