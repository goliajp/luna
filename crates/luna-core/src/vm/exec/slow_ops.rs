//! The opcodes the fast loop hands back to the loop head.

use super::*;

mod tail_call;

/// The ops below run in place in [`Vm::run_slow`] when optimised, and each
/// in a function of its own otherwise: the frame under a call one of them
/// makes then holds none of the other ops' locals.
macro_rules! jmp_close_op {
    ($vm:ident, $inst:ident, $cl:ident, $base:ident) => {{
        $vm.close_from($base + $inst.a() - 1);
        let off = $inst.jump_offset();
        // the pc after the jump's own
        let pc = $vm.pc_of_top() - 1;
        $vm.add_pc(off);
        // a loop's back-edge counts toward recording it, as a `Jmp`'s
        if off < 0 && $vm.jit.trace_enabled {
            let target = crate::vm::isa::Inst::branch_target(pc, off);
            if $vm.back_edge_hot($cl.proto, target) {
                $vm.trace_start_at_back_edge($cl, $base, target, None);
            }
        }
    }};
}

macro_rules! close_op {
    ($vm:ident, $inst:ident, $cl:ident, $base:ident, $entry_depth:ident) => {{
        // the handlers run through the interpreter loop, so one
        // can yield, above the whole register window: a goto out
        // of a nested loop leaves the top at the inner body's,
        // below a variable still to close (locals.lua:1219)
        $vm.top = $vm.top.max($base + $cl.proto.max_stack as u32);
        let _ = $vm.begin_close($base + $inst.a(), None, AfterClose::Block, $entry_depth)?;
    }};
}

macro_rules! tfor_call_op {
    ($vm:ident, $inst:ident, $base:ident) => {{
        let lay = $inst.op().for_layout().expect("a loop op");
        let abs = $base + $inst.a();
        let need = (abs + lay.call_end()) as usize;
        $vm.grow_stack(need)?;
        // the iterator, the state and the control, copied to where
        // the call runs (the control first: in 5.5 it is there)
        let call = (abs + lay.var()) as usize;
        $vm.stack[call + 2] = $vm.stack[(abs + lay.control()) as usize];
        $vm.stack[call + 1] = $vm.stack[(abs + 1) as usize];
        $vm.stack[call] = $vm.stack[abs as usize];
        let nvars = $inst.c() as i32;
        if !$vm.begin_call(call as u32, Some(2), nvars, false)? {
            // 5.1's `luaD_call` of a C iterator checks for a
            // collection when it returns
            $vm.call_return_checkpoint(call as u32 + nvars as u32)?;
        }
    }};
}

impl Vm {
    /// Run an opcode the fast loop handed back. `Some` carries the results
    /// once the call that `entry_depth` started has returned. A function
    /// of its own when unoptimised, off the stack under the fast loop.
    #[cfg_attr(luna_hot, inline(always))]
    pub(super) fn run_slow(
        &mut self,
        inst: Inst,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        // the fast loop may have called or returned into another frame
        let &Frame {
            closure: cl, base, ..
        } = self.top_frame();
        match inst.op() {
            Op::LoadKx
            | Op::NewTable
            | Op::SetList
            | Op::Pow
            | Op::Concat
            | Op::ForPrep
            | Op::ForPrep55
            | Op::TForPrep
            | Op::TForPrep53
            | Op::TForPrep55
            | Op::Closure
            | Op::Vararg
            | Op::GetVarg => self.run_frame_op(inst)?,
            // 5.2 / 5.3 jumps that close: rare enough to leave the fast loop
            #[cfg(luna_hot)]
            Op::JmpClose | Op::JmpCloseBack => jmp_close_op!(self, inst, cl, base),
            #[cfg(not(luna_hot))]
            Op::JmpClose | Op::JmpCloseBack => self.op_jmp_close(inst, cl, base),
            #[cfg(luna_hot)]
            Op::Close => close_op!(self, inst, cl, base, entry_depth),
            #[cfg(not(luna_hot))]
            Op::Close => self.op_close(inst, cl, base, entry_depth)?,
            Op::Tbc => {
                self.register_tbc(base + inst.a())?;
            }
            Op::TailCall => return self.op_tail_call(inst, base, entry_depth),
            Op::Return | Op::Return0 | Op::Return1 => {
                return self.op_return(inst, base, entry_depth);
            }
            #[cfg(luna_hot)]
            Op::TForCall | Op::TForCall53 | Op::TForCall55 => tfor_call_op!(self, inst, base),
            #[cfg(not(luna_hot))]
            Op::TForCall | Op::TForCall53 | Op::TForCall55 => self.op_tfor_call(inst, base)?,
            Op::ExtraArg => unreachable!("EXTRAARG executed directly"),
            op => unreachable!("{op:?} is run by the fast loop"),
        }
        Ok(None)
    }

    #[cfg(not(luna_hot))]
    fn op_jmp_close(&mut self, inst: Inst, cl: Gc<LuaClosure>, base: u32) {
        jmp_close_op!(self, inst, cl, base)
    }

    #[cfg(not(luna_hot))]
    fn op_close(
        &mut self,
        inst: Inst,
        cl: Gc<LuaClosure>,
        base: u32,
        entry_depth: usize,
    ) -> Result<(), LuaError> {
        close_op!(self, inst, cl, base, entry_depth);
        Ok(())
    }

    #[cfg(not(luna_hot))]
    fn op_tfor_call(&mut self, inst: Inst, base: u32) -> Result<(), LuaError> {
        tfor_call_op!(self, inst, base);
        Ok(())
    }

    #[cfg_attr(luna_hot, inline(always))]
    fn op_return(
        &mut self,
        inst: Inst,
        base: u32,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        let (abs_a, nret) = match inst.op() {
            Op::Return0 => (base + inst.a(), 0),
            Op::Return1 => (base + inst.a(), 1),
            _ => {
                let abs_a = base + inst.a();
                let nret = if inst.b() == 0 {
                    self.top - abs_a
                } else {
                    inst.b() - 1
                };
                (abs_a, nret)
            }
        };
        // `__close` handlers run above the top, which a fixed-count return
        // may leave below its results: raised past them, so no handler call
        // clobbers one (and the results survive a yield in the close)
        self.top = self.top.max(abs_a + nret);
        // a return that closes raises PUC's top to `ci->top` first, where
        // `luaF_close` calls the methods
        if inst.k() {
            self.top = self.top.max(self.lua_window_end());
        }
        if matches!(inst.op(), Op::Return0 | Op::Return1)
            && !matches!(
                self.return_fast::<true>(base, abs_a, nret, entry_depth, inst.k()),
                call::fast::Returned::No
            )
        {
            // done: the caller's frame is on top
        } else if let Some(vals) = self.begin_close(
            base,
            None,
            AfterClose::Return {
                abs_a,
                nret,
                from_native: false,
            },
            entry_depth,
        )? {
            return Ok(Some(vals));
        }
        Ok(None)
    }
}
