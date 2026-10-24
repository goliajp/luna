//! Making closures: `OP_CLOSURE` and the closures other code builds from a proto.

use super::*;

impl Vm {
    /// OP_CLOSURE: a new closure over the proto's upvalue descriptors.
    pub(super) fn op_closure(
        &mut self,
        inst: Inst,
        cl: Gc<LuaClosure>,
        base: u32,
    ) -> Result<(), LuaError> {
        let nc = self.closure_over(cl, inst.bx() as usize, base, true)?;
        self.set_r(base, inst.a(), Value::Closure(nc));
        self.checkpoint_after_op(inst, base)?;
        Ok(())
    }

    /// OP_CLOSURE for compiled code, in a frame of `cl` whose registers
    /// start at `base`: `None` when an allocation fails (compiled code may
    /// hold new objects only in registers, so no emergency collection
    /// runs; the interpreter then makes the closure).
    pub(crate) fn jit_closure(
        &mut self,
        cl: Gc<LuaClosure>,
        idx: usize,
        base: u32,
    ) -> Option<Gc<LuaClosure>> {
        self.closure_over(cl, idx, base, false).ok()
    }

    /// A new closure of `cl.proto.protos[idx]` in a frame of `cl` whose
    /// registers start at `base`; `collect`: an allocation that fails may
    /// run an emergency collection.
    fn closure_over(
        &mut self,
        cl: Gc<LuaClosure>,
        idx: usize,
        base: u32,
        collect: bool,
    ) -> Result<Gc<LuaClosure>, LuaError> {
        use crate::runtime::function::{INLINE_UPVALS_N, Upvalue};
        let proto = cl.proto.protos[idx];
        let n_ups = proto.upvals.len();
        // most closures capture one or two variables: their upvalues are
        // gathered on the stack
        let mut stack_buf: [std::mem::MaybeUninit<Gc<Upvalue>>; INLINE_UPVALS_N] =
            [std::mem::MaybeUninit::uninit(); INLINE_UPVALS_N];
        let mut heap_buf = crate::runtime::mem::LVec::new(self.heap.mem());
        let use_inline = n_ups <= INLINE_UPVALS_N;
        if !use_inline {
            heap_buf.reserve_exact(n_ups)?;
        }
        // PUC makes the closure first, then the upvalues it finds no open
        // one for (`luaF_findupval`): the new ones are counted after it
        let r = self.uncounted(|vm| {
            vm.gather_upvals(cl, proto, base, collect, &mut stack_buf, &mut heap_buf)
        });
        r?;
        let ups: &[Gc<Upvalue>] = if use_inline {
            // SAFETY: `gather_upvals` initialised the first `n_ups` slots
            unsafe { std::slice::from_raw_parts(stack_buf.as_ptr() as *const Gc<Upvalue>, n_ups) }
        } else {
            &heap_buf[..]
        };
        let nc = if collect {
            self.try_closure_from_proto(proto, ups)?
        } else {
            self.jit_closure_from_proto(proto, ups)
                .ok_or(LuaError(Value::Nil))?
        };
        // (5.1's environment is not an upvalue in PUC: never counted)
        let upval = self.heap.puc_size(crate::runtime::puc::PucObj::Upval);
        for (i, uv) in ups.iter().enumerate() {
            if self.version() > LuaVersion::Lua51 || i != proto.env_upval_idx as usize {
                let h = uv.hdr();
                self.heap
                    .acct_promote_obj(h, crate::runtime::heap::PucKind::Upval, upval);
            }
        }
        Ok(nc)
    }

    /// The upvalues of a new closure of `proto` in a frame of `cl` at
    /// `base`, into the first `proto.upvals.len()` slots of `inline` (when
    /// they fit) or onto `heap`. 5.1 gives the closure its own `_ENV`
    /// cell: luna shares the creator's closed one (`setfenv` never writes
    /// a cell in place but gives the function a new one, so sharing reads
    /// as PUC's copy), and snapshots an open one, which can change under
    /// the child.
    fn gather_upvals(
        &mut self,
        cl: Gc<LuaClosure>,
        proto: Gc<crate::runtime::function::Proto>,
        base: u32,
        collect: bool,
        inline: &mut [std::mem::MaybeUninit<Gc<crate::runtime::function::Upvalue>>],
        heap: &mut crate::runtime::mem::LVec<Gc<crate::runtime::function::Upvalue>>,
    ) -> Result<(), LuaError> {
        let use_inline = proto.upvals.len() <= inline.len();
        let e51 = (self.version() <= LuaVersion::Lua51).then_some(proto.env_upval_idx as usize);
        for (i, d) in proto.upvals.iter().enumerate() {
            // a new upvalue goes on the open list, a root
            let mut uv = if !d.in_stack {
                cl.upvals()[d.index as usize]
            } else if collect {
                self.try_find_or_create_upval(base + d.index as u32)?
            } else {
                self.jit_find_or_create_upval(base + d.index as u32)
                    .ok_or(LuaError(Value::Nil))?
            };
            if e51 == Some(i)
                && let UpvalState::Open { slot, thread } = uv.state()
            {
                let cur = self.read_slot(slot, thread);
                // young until the closure holds it
                uv = if collect {
                    self.retry_oom(|vm| vm.heap.try_new_upvalue(UpvalState::Closed(cur)))?
                } else {
                    self.heap
                        .try_new_upvalue(UpvalState::Closed(cur))
                        .map_err(|_| LuaError(Value::Nil))?
                };
            }
            if use_inline {
                inline[i] = std::mem::MaybeUninit::new(uv);
            } else {
                heap.push(uv)?;
            }
        }
        Ok(())
    }

    /// A closure of `proto` over `ups`. PUC 5.2 / 5.3 `getcached`: the
    /// Proto remembers its last closure and reuses it when every upvalue is
    /// the same Upvalue object, so `function() return outer end` built twice
    /// compares equal while a per-iteration loop variable defeats it. 5.1
    /// and 5.4+ always build a new one. A memory error when the closure
    /// cannot be made; the upvalues are held by the caller (on the open
    /// list, or young), so an emergency collection keeps them.
    pub(crate) fn try_closure_from_proto(
        &mut self,
        proto: Gc<crate::runtime::function::Proto>,
        ups: &[Gc<crate::runtime::function::Upvalue>],
    ) -> Result<Gc<LuaClosure>, LuaError> {
        let cached = self.version().has_closure_cache();
        if let Some(c) = proto.cache.get().filter(|c| {
            cached
                && c.upvals().len() == ups.len()
                && c.upvals()
                    .iter()
                    .zip(ups)
                    .all(|(a, b)| a.as_ptr() == b.as_ptr())
        }) {
            return Ok(c);
        }
        let n = self.retry_oom(|vm| vm.heap.try_new_closure(proto, ups))?;
        if cached {
            // PUC `luaC_protobarrier`: a black prototype is traversed again,
            // so the atomic step sees its cache (a weak reference) and
            // clears it if the closure dies
            self.heap.barrier_back(proto);
            proto.cache.set(Some(n));
        }
        Ok(n)
    }

    /// 5.1 `setfenv` on a Lua function: give `cl` a new `_ENV` cell (slot
    /// `idx`) holding `env`. A 5.1 env cell is never written in place —
    /// closures share their creator's cell, and replacing it here is what
    /// keeps the change to this one function. Raises a memory error when
    /// the new cell cannot be made.
    pub(crate) fn try_set_closure_env(
        &mut self,
        cl: Gc<LuaClosure>,
        idx: usize,
        env: Gc<Table>,
    ) -> Result<(), LuaError> {
        // luna's own cell: PUC keeps a 5.1 environment in the closure
        let off = self.heap.counting_off();
        let uv = self.retry_oom(|vm| {
            vm.heap
                .try_new_upvalue(UpvalState::Closed(Value::Table(env)))
        });
        self.heap.counting_on(off);
        let uv = uv?;
        // SAFETY: `cl` is a live closure (see `Gc`), and no reference into
        // it is held across this write
        unsafe { cl.as_mut() }.upvals_mut()[idx] = uv;
        // a black closure takes the new cell as PUC's takes the table
        // (`luaC_objbarrier`)
        self.heap.barrier_back(cl);
        Ok(())
    }
}
