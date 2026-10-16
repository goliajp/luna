//! The opcodes that stay in the running frame but allocate, grow the stack
//! or may call a metamethod: both the loop head and the fast loop run them
//! here, the fast loop without leaving for the loop head.

use super::*;

mod closure;

/// [`Vm::jit_gc_due`]'s op: the one before the pc compiled code resumes at.
pub const JIT_GC_PREV: u32 = u32::MAX;
/// [`Vm::jit_gc_due`]'s op: the call that entered the frame it resumes in.
pub const JIT_GC_CALL: u32 = u32::MAX - 1;

impl Vm {
    #[inline(never)]
    pub(super) fn run_frame_op(&mut self, inst: Inst) -> Result<(), LuaError> {
        let &Frame {
            closure: cl,
            base,
            func_slot,
            n_varargs,
            ..
        } = self.top_frame();
        match inst.op() {
            Op::LoadKx => {
                let extra = cl.proto.code[self.pc_of_top() as usize];
                self.bump_pc();
                let v = cl.proto.consts[extra.ax() as usize];
                self.set_r(base, inst.a(), v);
            }
            Op::NewTable => {
                let Some((asize, hsize)) =
                    crate::runtime::table::new_table_sizes(inst.b(), inst.c(), inst.k())
                else {
                    return Err(self.rt_err("table overflow"));
                };
                let t = self.alloc_table()?;
                if asize != 0 || hsize != 0 {
                    // the table is young until it is stored below, so an
                    // emergency collection keeps it
                    let r = self.table_op(|vm| {
                        // SAFETY: `t` was made just above; this is its only
                        // reference, for the one resize
                        unsafe { t.as_mut() }.resize(&mut vm.heap, asize, hsize)
                    });
                    if let Err(e) = r {
                        return Err(self.table_error(e));
                    }
                }
                self.set_r(base, inst.a(), Value::Table(t));
                self.checkpoint_after_op(inst, base);
            }
            Op::SetList => {
                let a = inst.a();
                let abs_a = base + a;
                // only `debug.setlocal` or crafted bytecode can put a
                // non-table here; PUC crashes, luna raises
                let t = match self.r(base, a) {
                    Value::Table(t) => t,
                    v => return Err(self.type_err("index", v)),
                };
                let n = if inst.b() == 0 {
                    self.top - (abs_a + 1)
                } else {
                    inst.b()
                };
                let offset = if inst.k() {
                    let extra = cl.proto.code[self.pc_of_top() as usize];
                    self.bump_pc();
                    extra.ax() as i64
                } else {
                    inst.c() as i64
                };
                // one barrier covers every store this op does — PUC's
                // `luaC_barrierback_` once-per-table optimisation; taken
                // first, its flag test also refuses a read-only table
                // (only one marked from outside between its constructor
                // and this op, by a debug hook, can be)
                if !self.heap.store_barrier(t) {
                    return Err(self.table_error(TableError::ReadOnly));
                }
                let last = offset as u64 + u64::from(n);
                let r = self.table_op(|vm| {
                    // SAFETY: `t` is a live table (see `Gc`), and no
                    // reference into it is held across the call
                    unsafe { t.as_mut() }.reserve_list(&mut vm.heap, last)
                });
                match r {
                    Err(TableError::Overflow) => return Err(self.rt_err("table overflow")),
                    Err(e) => return Err(self.table_error(e)),
                    Ok(()) => {}
                }
                // SAFETY: `t` is a live table (see `Gc`), and no reference
                // into it is held across these stores
                let tb = unsafe { t.as_mut() };
                for i in 0..n {
                    tb.set_list_slot(offset as usize + i as usize, self.r(base, a + 1 + i));
                }
            }
            Op::Pow => {
                let (l, r) = (self.r(base, inst.b()), self.r(base, inst.c()));
                self.arith_slow(inst.a(), base, ArithOp::Pow, l, r)?
            }
            Op::Concat => {
                // right-associative fold over the operands, in place on the
                // stack so a yielding __concat can suspend
                let (first, out) = inst.concat_operands();
                self.top = base + first + inst.b();
                if self.concat_run(base + first, base + out)? {
                    self.concat_checkpoint(base + first, base + out);
                }
            }
            Op::ForPrep | Op::ForPrep55 => self.for_prep(inst, base)?,
            Op::TForPrep | Op::TForPrep53 | Op::TForPrep55 => {
                let a = base + inst.a();
                // 5.5's list left the closing value above the control:
                // swap them (PUC `OP_TFORPREP`)
                if inst.op() == Op::TForPrep55 {
                    self.stack.swap((a + 2) as usize, (a + 3) as usize);
                }
                if let Some(c) = inst.op().for_layout().and_then(|l| l.closing()) {
                    self.register_tbc(a + c)?;
                }
                self.add_pc(inst.bx() as i32);
            }
            Op::Closure => self.op_closure(inst, cl, base)?,
            Op::Vararg => {
                let abs_a = base + inst.a();
                let wanted = inst.c() as i32 - 1;
                // A materialized named vararg lives in func_slot (its writes
                // must be visible to `...`); otherwise spread the extra args
                // straight off the stack, where they sit just below `base`.
                let vt = match self.stack[func_slot as usize] {
                    Value::Table(t) => Some(t),
                    _ => None,
                };
                let n = match vt {
                    Some(t) => {
                        let n_key = self.str_value(b"n")?;
                        // PUC getnumargs: a named vararg `t.n` set out of the
                        // integer range [0, INT_MAX/2] is rejected here
                        match t.get(n_key) {
                            Value::Int(n) if (n as u64) <= (i32::MAX as u64 / 2) => n as u32,
                            _ => return Err(self.rt_err("vararg table has no proper 'n'")),
                        }
                    }
                    None => n_varargs,
                };
                let count = if wanted < 0 { n } else { wanted as u32 };
                // all of them (PUC `luaT_getvarargs`) are checked for above
                // the frame's top (`L->top` is `ci->top` while it runs); a
                // named vararg's `n` can be set to anything up to INT_MAX/2,
                // which the stack cannot hold
                if wanted < 0 {
                    let gc = self.version == LuaVersion::Lua54;
                    self.check_lua_stack(self.lua_window_end(), count, gc)?;
                }
                let need = (abs_a + count) as usize;
                self.grow_stack(need)?;
                for i in 0..count {
                    let v = if i >= n {
                        Value::Nil
                    } else if let Some(t) = vt {
                        t.get_int(i as i64 + 1)
                    } else {
                        self.stack[(base - n_varargs + i) as usize]
                    };
                    self.stack[(abs_a + i) as usize] = v;
                }
                if wanted < 0 {
                    self.top = abs_a + count;
                }
            }
            Op::GetVarg => {
                // materialize the vararg table (PUC table.pack shape) from the
                // stack varargs — used when the named vararg is written /
                // escapes / is `_ENV`. It is kept BOTH in func_slot (so `...`
                // sees later writes) and in the local register R[A].
                let n = n_varargs;
                // the table is young until it is stored below, so an
                // emergency collection keeps it; PUC `createvarargtab`: an
                // array part of exactly `n`
                let t = self.alloc_table()?;
                let r = self.table_op(|vm| {
                    // SAFETY: `t` was made just above; this is its only
                    // reference, for the one resize
                    unsafe { t.as_mut() }.resize(&mut vm.heap, n as usize, 1)
                });
                if let Err(e) = r {
                    return Err(self.table_error(e));
                }
                for i in 0..n {
                    let v = self.stack[(base - n_varargs + i) as usize];
                    // SAFETY: as above, for this one store
                    unsafe { t.as_mut() }.set_list_slot(i as usize, v);
                }
                let n_key = self.str_value(b"n")?;
                self.raw_set(t, n_key, Value::Int(n as i64))?;
                self.stack[func_slot as usize] = Value::Table(t);
                self.set_r(base, inst.a(), Value::Table(t));
            }
            Op::ShlK | Op::ShrK | Op::EqKK | Op::LtKK | Op::LeKK => {
                self.const_frame_op(inst, cl, base)?
            }
            op if op.arith_kk_op().is_some() => self.const_frame_op(inst, cl, base)?,
            op => unreachable!("{op:?} is not a frame op"),
        }
        Ok(())
    }

    /// The check for a collection after `inst` (NEWTABLE, CLOSURE or
    /// CONCAT) finished in the running Lua function, its registers at
    /// `base`, with each dialect's stack top: 5.1 keeps the whole frame
    /// (CLOSURE puts the closure straight into its variable, which may lie
    /// below other live registers), the later dialects up to the result.
    pub(super) fn checkpoint_after_op(&mut self, inst: Inst, base: u32) {
        let v51 = self.version() == LuaVersion::Lua51;
        let top = match inst.op() {
            Op::Concat => {
                let (first, out) = inst.concat_operands();
                return self.concat_checkpoint(base + first, base + out);
            }
            Op::Closure if v51 => {
                let max = self.top_frame().closure.proto.max_stack as u32;
                base + max.max(inst.a() + 1)
            }
            _ if v51 && inst.op() == Op::NewTable => self.frame_top(),
            _ => base + inst.a() + 1,
        };
        self.checkpoint(top);
    }

    /// Compiled code left with a step due after the op at `op_pc`
    /// ([`JIT_GC_PREV`]: the one before the running frame's pc;
    /// [`JIT_GC_CALL`]: the call that entered the running frame, which 5.2
    /// checks with the frame's top): the check that op makes.
    pub(crate) fn checkpoint_after_jit(&mut self, op_pc: u32) {
        if op_pc == JIT_GC_CALL {
            return self.checkpoint(self.frame_top());
        }
        let (cl, base) = (self.top_frame().closure, self.top_frame().base);
        let pc = if op_pc == JIT_GC_PREV {
            self.pc_of_top() - 1
        } else {
            op_pc
        };
        self.checkpoint_after_op(cl.proto.code[pc as usize], base);
    }

    /// Compiled code just allocated (or 5.2 entered a call): whether a
    /// collection step is due, noted with the op whose check it is
    /// (`op_pc`, as [`Vm::checkpoint_after_jit`] takes it) for the exit it
    /// then takes to have the interpreter check.
    #[doc(hidden)]
    pub fn jit_gc_due(&mut self, op_pc: u32) -> bool {
        let due = self.step_due() && self.jit.pending_err.is_none();
        if due {
            self.jit.gc_exit = Some(op_pc);
        }
        due
    }

    /// Whether a check for a collection now would step it.
    pub(crate) fn step_due(&self) -> bool {
        self.heap.gc_due() && !self.gc_finalizing
    }
}
