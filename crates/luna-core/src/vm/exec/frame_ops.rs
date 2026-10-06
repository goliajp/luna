//! The opcodes that stay in the running frame but allocate, grow the stack
//! or may call a metamethod: both the loop head and the fast loop run them
//! here, the fast loop without leaving for the loop head.

use super::*;

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
                let Some(t) = self.heap.new_table_presized(inst.b(), inst.c(), inst.k()) else {
                    return Err(self.rt_err("table overflow"));
                };
                self.set_r(base, inst.a(), Value::Table(t));
                self.maybe_collect_garbage(base + inst.a() + 1);
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
                // SAFETY: `t` is a live table (see `Gc`), and no reference
                // into it is held across these calls
                let tb = unsafe { t.as_mut() };
                if tb
                    .reserve_list(&mut self.heap, offset as u64 + u64::from(n))
                    .is_err()
                {
                    return Err(self.rt_err("table overflow"));
                }
                for i in 0..n {
                    tb.set_list_slot(offset as usize + i as usize, self.r(base, a + 1 + i));
                }
                // the element temps above the table are now consumed
                self.maybe_collect_garbage(base + a + 1);
            }
            Op::Pow => {
                let (l, r) = (self.r(base, inst.b()), self.r(base, inst.c()));
                self.arith_slow(inst.a(), base, ArithOp::Pow, l, r, false)?
            }
            Op::Concat => {
                // right-associative fold over operands at base+a .. base+a+n,
                // in place on the stack so a yielding __concat can suspend.
                let a = inst.a();
                let n = inst.b();
                self.top = base + a + n;
                self.concat_run(base + a)?;
            }
            Op::ForPrep => self.for_prep(inst, base)?,
            Op::TForPrep => {
                // the 4th control slot is the iterator's closing value
                self.register_tbc(base + inst.a() + 3)?;
                self.add_pc(inst.bx() as i32);
            }
            Op::Closure => self.op_closure(inst, cl, base),
            Op::Vararg => {
                let abs_a = base + inst.a();
                let wanted = inst.c() as i32 - 1;
                // A materialized named vararg lives in func_slot (its writes
                // must be visible to `...`); otherwise spread the extra args
                // straight off the stack at func_slot+1 .. +n_varargs.
                let vt = match self.stack[func_slot as usize] {
                    Value::Table(t) => Some(t),
                    _ => None,
                };
                let n = match vt {
                    Some(t) => {
                        let n_key = Value::Str(self.heap.intern(b"n"));
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
                // a named vararg's `n` can be set to anything up to
                // INT_MAX/2; PUC's `luaD_checkstack` refuses what the
                // stack cannot hold
                if abs_a + count > MAX_LUA_STACK {
                    return Err(self.rt_err("stack overflow"));
                }
                let need = (abs_a + count) as usize;
                if self.stack.len() < need {
                    self.grow_stack_or_abort(need);
                }
                for i in 0..count {
                    let v = if i >= n {
                        Value::Nil
                    } else if let Some(t) = vt {
                        t.get_int(i as i64 + 1)
                    } else {
                        self.stack[(func_slot + 1 + i) as usize]
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
                let t = self.heap.new_table();
                {
                    // SAFETY: `t` was allocated above and is held only by this
                    // local; `tm` is the only reference into it, and the heap
                    // calls made while it lives (`set_int`, `intern`, `set`)
                    // do not collect
                    let tm = unsafe { t.as_mut() };
                    // PUC `createvarargtab`: an array part of exactly `n`
                    tm.resize(&mut self.heap, n as usize, 1);
                    for i in 0..n {
                        tm.set_list_slot(i as usize, self.stack[(func_slot + 1 + i) as usize]);
                    }
                    let n_key = Value::Str(self.heap.intern(b"n"));
                    tm.set(&mut self.heap, n_key, Value::Int(n as i64))
                        .expect("'n' is a valid key");
                }
                // once-per-table barrier (mirror SETLIST): t is born BLACK
                // during Propagate; the bulk inserts above don't barrier.
                self.heap.barrier_back(t);
                self.stack[func_slot as usize] = Value::Table(t);
                self.set_r(base, inst.a(), Value::Table(t));
            }
            op => unreachable!("{op:?} is not a frame op"),
        }
        Ok(())
    }

    /// OP_CLOSURE: a new closure over the proto's upvalue descriptors.
    fn op_closure(&mut self, inst: Inst, cl: Gc<LuaClosure>, base: u32) {
        let proto = cl.proto.protos[inst.bx() as usize];
        let n_ups = proto.upvals.len();
        // Build upvals on the stack for small
        // closures, skipping the per-call Vec/Box alloc
        // that closure_alloc's 10k iters pay. INLINE_UPVALS_N
        // = 2 covers most Lua source (1 captured local, or
        // _ENV + a single capture). Beyond that, fall back
        // to a heap Vec.
        use crate::runtime::function::INLINE_UPVALS_N;
        let mut stack_buf: [std::mem::MaybeUninit<Gc<crate::runtime::function::Upvalue>>;
            INLINE_UPVALS_N] = [std::mem::MaybeUninit::uninit(); INLINE_UPVALS_N];
        let mut heap_buf: Vec<Gc<crate::runtime::function::Upvalue>> = Vec::new();
        let use_inline = n_ups <= INLINE_UPVALS_N;
        if !use_inline {
            heap_buf.reserve_exact(n_ups);
        }
        for (i, d) in proto.upvals.iter().enumerate() {
            let uv = if d.in_stack {
                self.find_or_create_upval(base + d.index as u32)
            } else {
                cl.upvals()[d.index as usize]
            };
            if use_inline {
                stack_buf[i] = std::mem::MaybeUninit::new(uv);
            } else {
                heap_buf.push(uv);
            }
        }
        // Tiny shim around the two paths so the 5.1 _ENV
        // clone + cache check below see one uniform
        // `&mut [Gc<Upvalue>]`. The stack_buf slice points
        // into the local frame (still valid through the
        // rest of this Op::Closure handler).
        let ups: &mut [Gc<crate::runtime::function::Upvalue>] = if use_inline {
            // SAFETY: the first n_ups slots of stack_buf
            // were initialised above; we hand out a slice
            // covering exactly them.
            unsafe {
                std::slice::from_raw_parts_mut(
                    stack_buf.as_mut_ptr() as *mut Gc<crate::runtime::function::Upvalue>,
                    n_ups,
                )
            }
        } else {
            &mut heap_buf[..]
        };
        // PUC 5.1 had per-function environments: every Lua
        // function carried its own `env` slot, snapshotted from
        // the creating function's env at closure time, so a
        // `setfenv` on one closure never bled into a sibling.
        // luna shares the creator's closed `_ENV` cell instead of
        // copying it: `setfenv` never writes a cell in place but gives
        // the target a new one (`set_closure_env`), so sharing reads
        // the same as a snapshot. An open cell can change under the
        // child, so that one is still snapshotted.
        if self.version() <= LuaVersion::Lua51 && proto.env_upval_idx != u8::MAX {
            let i = proto.env_upval_idx as usize;
            if let UpvalState::Open { slot, thread } = ups[i].state() {
                let cur = self.read_slot(slot, thread);
                ups[i] = self.heap.new_upvalue(UpvalState::Closed(cur));
            }
        }
        let nc = self.closure_from_proto(proto, ups);
        self.set_r(base, inst.a(), Value::Closure(nc));
        self.maybe_collect_garbage(base + inst.a() + 1);
    }

    /// A closure of `proto` over `ups`. PUC 5.2 / 5.3 `getcached`: the
    /// Proto remembers its last closure and reuses it when every upvalue is
    /// the same Upvalue object, so `function() return outer end` built twice
    /// compares equal while a per-iteration loop variable defeats it. 5.1
    /// and 5.4+ always build a new one.
    #[doc(hidden)]
    pub fn closure_from_proto(
        &mut self,
        proto: Gc<crate::runtime::function::Proto>,
        ups: &[Gc<crate::runtime::function::Upvalue>],
    ) -> Gc<LuaClosure> {
        let cached = self.version().has_closure_cache();
        if let Some(c) = proto.cache.get().filter(|c| {
            cached
                && c.upvals().len() == ups.len()
                && c.upvals()
                    .iter()
                    .zip(ups)
                    .all(|(a, b)| a.as_ptr() == b.as_ptr())
        }) {
            return c;
        }
        let n = self.heap.new_closure_inline(proto, ups);
        if cached {
            proto.cache.set(Some(n));
        }
        n
    }

    /// 5.1 `setfenv` on a Lua function: give `cl` a new `_ENV` cell (slot
    /// `idx`) holding `env`. A 5.1 env cell is never written in place —
    /// closures share their creator's cell, and replacing it here is what
    /// keeps the change to this one function.
    pub(crate) fn set_closure_env(&mut self, cl: Gc<LuaClosure>, idx: usize, env: Gc<Table>) {
        let uv = self.heap.new_upvalue(UpvalState::Closed(Value::Table(env)));
        // SAFETY: `cl` is a live closure (see `Gc`), and no reference into
        // it is held across this write
        unsafe { cl.as_mut() }.upvals_mut()[idx] = uv;
        // a cell born during propagation is black: its value needs the barrier
        self.barrier_forward_upvalue(uv, Value::Table(env));
    }
}
