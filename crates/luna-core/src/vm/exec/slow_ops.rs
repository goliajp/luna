//! The opcodes the fast loop hands back to the loop head.

use super::*;

mod tail_call;

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
            Op::JmpClose | Op::JmpCloseBack => {
                self.close_from(base + inst.a() - 1);
                let off = inst.jump_offset();
                self.add_pc(off);
                // a loop's back-edge counts toward recording it, as a `Jmp`'s
                if off < 0 && self.jit.trace_enabled {
                    let proto = cl.proto;
                    let c = proto.trace_hot_count.get();
                    if c < u32::MAX / 2 {
                        proto.trace_hot_count.set(c + 1);
                    }
                    let target = self.pc_of_top();
                    if self.jit.loop_hot_tick(&proto, target) {
                        self.trace_start_at_back_edge(cl, base, target, None);
                    }
                }
            }
            Op::Close => {
                // the handlers run through the interpreter loop, so one
                // can yield, above the whole register window: a goto out
                // of a nested loop leaves the top at the inner body's,
                // below a variable still to close (locals.lua:1219)
                self.top = self.top.max(base + cl.proto.max_stack as u32);
                let _ = self.begin_close(base + inst.a(), None, AfterClose::Block, entry_depth)?;
            }
            Op::Tbc => {
                self.register_tbc(base + inst.a())?;
            }
            Op::TailCall => return self.op_tail_call(inst, base, entry_depth),
            Op::Return | Op::Return0 | Op::Return1 => {
                return self.op_return(inst, base, entry_depth);
            }
            Op::TForCall | Op::TForCall53 | Op::TForCall55 => {
                let lay = inst.op().for_layout().expect("a loop op");
                let abs = base + inst.a();
                let need = (abs + lay.call_end()) as usize;
                self.grow_stack(need)?;
                // the iterator, the state and the control, copied to where
                // the call runs (the control first: in 5.5 it is there)
                let call = (abs + lay.var()) as usize;
                self.stack[call + 2] = self.stack[(abs + lay.control()) as usize];
                self.stack[call + 1] = self.stack[(abs + 1) as usize];
                self.stack[call] = self.stack[abs as usize];
                let nvars = inst.c() as i32;
                if !self.begin_call(call as u32, Some(2), nvars, false)? {
                    // 5.1's `luaD_call` of a C iterator checks for a
                    // collection when it returns
                    self.call_return_checkpoint(call as u32 + nvars as u32)?;
                }
            }
            Op::ExtraArg => unreachable!("EXTRAARG executed directly"),
            op => unreachable!("{op:?} is run by the fast loop"),
        }
        Ok(None)
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
                call_fast::Returned::No
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
