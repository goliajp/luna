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
        let nc = self.closure_over(cl, inst.bx() as usize, base, Some(inst.a()))?;
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
        self.closure_over(cl, idx, base, None).ok()
    }

    /// A new closure of `cl.proto.protos[idx]` in a frame of `cl` whose
    /// registers start at `base`, for register `ra` (`None`: for compiled
    /// code, where an allocation that fails runs no emergency collection).
    ///
    /// PUC `pushclosure` makes the closure first, anchored in its register,
    /// and then finds or makes each upvalue (`luaF_findupval`), putting it
    /// in at once: a collection meanwhile keeps the ones found through the
    /// closure, and frees dead open upvalues not found yet.
    fn closure_over(
        &mut self,
        cl: Gc<LuaClosure>,
        idx: usize,
        base: u32,
        ra: Option<u32>,
    ) -> Result<Gc<LuaClosure>, LuaError> {
        let proto = cl.proto.protos[idx];
        let collect = ra.is_some();
        let cached = self.version().has_closure_cache();
        if cached && let Some(c) = self.cached_closure(cl, proto, base) {
            return Ok(c);
        }
        let n = proto.upvals.len();
        let nc = if collect {
            self.retry_oom(|vm| vm.heap.try_new_closure_to_fill(proto, n))?
        } else {
            self.heap
                .try_new_closure_to_fill(proto, n)
                .map_err(|_| LuaError(Value::Nil))?
        };
        if let Some(a) = ra {
            self.set_r(base, a, Value::Closure(nc));
        }
        self.fill_upvals(nc, cl, proto, base, collect)?;
        if cached {
            self.heap.cache_closure(proto, nc);
        }
        Ok(nc)
    }

    /// PUC 5.2 / 5.3 `getcached`: the closure `proto` remembers when each
    /// of its upvalues is the one a new closure in a frame of `cl` at
    /// `base` would get, so `function() return outer end` built twice
    /// compares equal while a per-iteration loop variable defeats it.
    fn cached_closure(
        &self,
        cl: Gc<LuaClosure>,
        proto: Gc<crate::runtime::function::Proto>,
        base: u32,
    ) -> Option<Gc<LuaClosure>> {
        let c = proto.cache.get()?;
        let same = c.upvals().len() == proto.upvals.len()
            && proto.upvals.iter().zip(c.upvals()).all(|(d, uv)| {
                if d.in_stack {
                    matches!(uv.state(), UpvalState::Open { slot, thread }
                        if slot == base + d.index as u32 && self.is_current_thread(thread))
                } else {
                    uv.ptr_eq(cl.upvals()[d.index as usize])
                }
            });
        same.then_some(c)
    }

    /// The upvalues of `nc`, a new closure of `proto` in a frame of `cl` at
    /// `base`, each found or made and put in at once. 5.1 gives the closure
    /// its own `_ENV` cell: luna shares the creator's closed one (`setfenv`
    /// never writes a cell in place but gives the function a new one, so
    /// sharing reads as PUC's copy), and snapshots an open one, which can
    /// change under the child; PUC has no such upvalue, so it is not
    /// counted.
    fn fill_upvals(
        &mut self,
        nc: Gc<LuaClosure>,
        cl: Gc<LuaClosure>,
        proto: Gc<crate::runtime::function::Proto>,
        base: u32,
        collect: bool,
    ) -> Result<(), LuaError> {
        let e51 = (self.version() <= LuaVersion::Lua51).then_some(proto.env_upval_idx as usize);
        for (i, d) in proto.upvals.iter().enumerate() {
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
                let made = self.uncounted(|vm| {
                    if collect {
                        vm.retry_oom(|vm| vm.heap.try_new_upvalue(UpvalState::Closed(cur)))
                    } else {
                        vm.heap
                            .try_new_upvalue(UpvalState::Closed(cur))
                            .map_err(|_| LuaError(Value::Nil))
                    }
                });
                uv = made?;
            }
            self.heap.put_upval(nc, i, uv);
        }
        Ok(())
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
        // a black closure takes the new cell as PUC's takes the table
        // (`luaC_objbarrier`)
        self.heap.set_closure_upval(cl, idx, uv);
        Ok(())
    }
}
