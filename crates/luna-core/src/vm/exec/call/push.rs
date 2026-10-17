//! Pushing a Lua function's frame.

use super::*;
use crate::runtime::Proto;

/// Where a call's frame goes: its base, the extra arguments a vararg
/// function keeps below it, and the stack it needs (`base + max_stack`).
#[derive(Clone, Copy)]
struct FrameAt {
    base: u32,
    n_varargs: u32,
    need: u32,
}

impl FrameAt {
    #[inline(always)]
    fn new(version: LuaVersion, proto: Gc<Proto>, func_slot: u32, nargs: u32) -> FrameAt {
        let nparams = proto.num_params as u32;
        let n_varargs = nargs.saturating_sub(nparams) * u32::from(proto.is_vararg);
        // a vararg frame sits where PUC puts it, so a recursion through
        // vararg functions takes the same stack: above the arguments, where
        // 5.1 to 5.3 copy the fixed parameters (`adjust_varargs`) and 5.4 on
        // the function too (`luaT_adjustvarargs`); the extras stay just
        // below the base, the slots they came from are dead
        let base = if proto.is_vararg {
            func_slot + 1 + n_varargs + nparams + u32::from(version >= LuaVersion::Lua54)
        } else {
            func_slot + 1
        };
        FrameAt {
            base,
            n_varargs,
            need: base + proto.max_stack as u32,
        }
    }
}

impl Vm {
    pub(in crate::vm::exec) fn push_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        from_c: bool,
    ) -> Result<(), LuaError> {
        // 5.1's `CallInfo` array is grown after the stack test
        let grow51 = self.g.frame_size != u32::MAX && self.frames_in_use() >= self.g.frame_size;
        let at = FrameAt::new(self.version, cl.proto, func_slot, nargs);
        // PUC's stack test (`luaD_precall`) wants the arguments' top plus
        // the frame's size below the stack's end: `need + nargs` is that for
        // a function of fixed parameters and no less for a vararg one, so a
        // test of it and the frame stack go out of line together. A function
        // a native calls back finds the stack shorter than its frame on most
        // calls: that room is made here, so the frame is entered by the one
        // copy of the code the common path runs
        if grow51
            || at.need + nargs >= self.g.stack_last
            || self.frames.len() == self.frames.capacity()
        {
            return self.push_frame_after_room(cl, func_slot, nargs, nresults, from_c);
        }
        self.grow_stack(at.need as usize)?;
        self.enter_frame(cl, func_slot, nargs, nresults, from_c, at)
    }

    /// [`Vm::push_frame`] near the stack's end, with the frame stack full
    /// or 5.1's `CallInfo` array full: PUC's exact stack test, 5.1's array
    /// grown, then room for `need` stack slots and one more frame,
    /// collecting in an emergency when memory runs out.
    #[cold]
    #[inline(never)]
    fn push_frame_after_room(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        from_c: bool,
    ) -> Result<(), LuaError> {
        let p = cl.proto;
        let (top, max) = (func_slot + 1 + nargs, p.max_stack as u32);
        self.check_lua_stack(top, max, true)?;
        if p.is_vararg {
            // a vararg function's stack is checked again: 5.2 above the
            // missing parameters (`adjust_varargs`), 5.4 and 5.5 for the
            // function's copy too (`luaT_adjustvarargs`, unless 5.5 builds
            // a vararg table instead)
            match self.version {
                // 5.1 makes the `arg` table of an old-style vararg function
                LuaVersion::Lua51 if p.has_compat_vararg_arg => {
                    let missing = u32::from(p.num_params).saturating_sub(nargs);
                    self.check_lua_stack(top + missing, max, false)?;
                }
                LuaVersion::Lua52 => {
                    let missing = u32::from(p.num_params).saturating_sub(nargs);
                    self.check_lua_stack(top + missing, max, false)?;
                }
                LuaVersion::Lua54 => self.check_lua_stack(top, max + 1, false)?,
                LuaVersion::Lua55 if !vararg_table_55(p) => {
                    self.check_lua_stack(top, max + 1, false)?
                }
                _ => {}
            }
        }
        // PUC 5.1 `inc_ci`, after `luaD_checkstack`
        if self.g.frame_size != u32::MAX && self.frames_in_use() >= self.g.frame_size {
            self.grow_frames()?;
        }
        let at = FrameAt::new(self.version, p, func_slot, nargs);
        self.grow_stack(at.need as usize)?;
        if self.frames.len() == self.frames.capacity() {
            self.retry_oom(|vm| vm.frames.reserve(1))?;
        }
        self.enter_frame(cl, func_slot, nargs, nresults, from_c, at)
    }

    /// Push the frame of a call [`FrameAt`] places, the stack and the frame
    /// stack having room for it.
    #[inline(always)]
    fn enter_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        from_c: bool,
        at: FrameAt,
    ) -> Result<(), LuaError> {
        let proto = cl.proto;
        let FrameAt {
            base, n_varargs, ..
        } = at;
        let nparams = proto.num_params as u32;
        let max = proto.max_stack as u32;
        // the frame stack has room: tested before, and pushed before any
        // write through the stack could make the compiler test again
        let cf = CallFrame::Lua(Frame {
            closure: cl,
            base,
            pc: 0,
            func_slot,
            nresults,
            hook_oldpc: u32::MAX,
            from_c,
            n_varargs,
            // single-shot consume: `close_slots` sets pending_tm before each
            // handler call; the next Lua frame born is that handler's.
            tm: self.pending_tm.take(),
            // `run_hook` sets `pending_is_hook` before dispatching the user
            // hook so its frame reports `namewhat = "hook"` via getinfo.
            is_hook: std::mem::take(&mut self.pending_is_hook),
            tailcalls: std::mem::take(&mut self.pending_tailcalls),
            ccmt: std::mem::take(&mut self.pending_ccmt),
        });
        frames_push_sync(&mut self.frames, &mut self.frames_top, &mut self.trap, cf);
        self.note_call_level();
        // 5.5 vararg layout (PUC luaT_adjustvarargs): the extra args stay on the
        // stack just below the new `base`, so a named vararg can be indexed
        // virtually without allocating a table.
        if proto.is_vararg && nargs > 0 {
            let s = (func_slot + 1) as usize;
            let kept = nargs.min(nparams) as usize;
            // `base + kept <= need` since `kept <= nparams <= max_stack`
            self.stack.copy_within(s..s + kept, base as usize);
            if n_varargs > 0 {
                let from = s + nparams as usize;
                let to = (base - n_varargs) as usize;
                self.stack.copy_within(from..from + n_varargs as usize, to);
            }
        }
        // Only missing parameters become nil (PUC `luaD_precall`): the code
        // writes the rest before reading it, the collector keeps it valid
        // (`clear_dead_stack`) and a trace checks only what it reads first.
        // 5.1 clears the whole window as PUC 5.1 does (its compiler drops a
        // leading `local x` LoadNil on that promise).
        let kept = nargs.saturating_sub(n_varargs).min(nparams);
        // 5.5's `luaT_adjustvarargs` sets the vararg parameter, the
        // register after the fixed ones, to nil
        let window = if self.version == LuaVersion::Lua51 {
            max
        } else if self.version == LuaVersion::Lua55 && proto.is_vararg {
            (nparams + 1).min(max)
        } else {
            nparams
        };
        let end = (base + window) as usize;
        // SAFETY: `at.need <= stack.len()` (tested before) and `base + kept <=
        // end <= at.need` since `kept <= nparams <= window <= max_stack`.
        unsafe {
            self.stack
                .get_unchecked_mut((base + kept) as usize..end)
                .fill(Value::Nil)
        };
        self.precall_checkpoint(base + max);
        // PUC 5.1 `LUAI_COMPAT_VARARG`: the hidden `arg` local (the slot at
        // `base + nparams`) gets `{ n = n_varargs, e1, e2, … }` from the extras
        // just below `base` (5.1 db.lua :279 reads `arg.n` from a line hook).
        if proto.has_compat_vararg_arg {
            // `adjust_varargs` checks for a collection first, the missing
            // parameters on the stack; the fixed ones are already copied up
            // to the new frame here (PUC copies them after), and stay live
            self.checkpoint((func_slot + 1 + nargs.max(nparams)).max(base + nparams));
            let arg_slot = (base + nparams) as usize;
            // young until stored in its slot below; `luaH_new(L, nvar, 1)`:
            // an array part of exactly the extras' count (bounded by the
            // stack, well below `MAX_ASIZE`), nil ones included
            let t = self.alloc_table()?;
            self.table_op(|vm| {
                // SAFETY: `t` was allocated above and is held only by this
                // local; the borrow covers the resize, which does not collect
                unsafe { t.as_mut() }.resize(&mut vm.heap, n_varargs as usize, 1)
            })?;
            for i in 0..n_varargs {
                let v = self.stack[(base - n_varargs + i) as usize];
                // SAFETY: `t` was allocated above and is held only by this
                // local; the reference lives for this one store
                unsafe { t.as_mut() }.set_list_slot(i as usize, v);
            }
            let nk = self.str_value(b"n")?;
            self.raw_set(t, nk, Value::Int(n_varargs as i64))?;
            self.stack[arg_slot] = Value::Table(t);
        }
        // PUC luaD_precall fires the "call" hook with the new frame current, so
        // a hook calling debug.getinfo(2) sees the entered function. For a Lua
        // callee, PUC `luaD_hookcall` passes `p->numparams` as ntransfer (only
        // fixed params count — extras already live below `base`).
        // A frame born via OP_TailCall fires "tail call" instead (PUC
        // luaD_pretailcall) and skips the matching "return" hook on exit.
        let is_tail = self
            .frames
            .last()
            .and_then(|f| f.lua())
            .is_some_and(|f| f.tailcalls > 0);
        self.hook_call_with(false, nparams, is_tail)?;
        Ok(())
    }
}

/// 5.5's `PF_VATAB`: the function's vararg parameter is a table its
/// `VARARGPREP` builds (its first instruction reads it into the
/// parameter's register).
fn vararg_table_55(p: Gc<Proto>) -> bool {
    p.code
        .first()
        .is_some_and(|i| i.op() == crate::vm::isa::Op::GetVarg && i.a() == u32::from(p.num_params))
}
