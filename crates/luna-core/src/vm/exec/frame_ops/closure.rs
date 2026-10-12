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
        let mut heap_buf = crate::runtime::mem::LVec::new(self.heap.mem());
        let use_inline = n_ups <= INLINE_UPVALS_N;
        if !use_inline {
            heap_buf.reserve_exact(n_ups)?;
        }
        for (i, d) in proto.upvals.iter().enumerate() {
            // a new upvalue goes on the open list, a root
            let uv = if d.in_stack {
                self.try_find_or_create_upval(base + d.index as u32)?
            } else {
                cl.upvals()[d.index as usize]
            };
            if use_inline {
                stack_buf[i] = std::mem::MaybeUninit::new(uv);
            } else {
                heap_buf.push(uv)?;
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
                // young until the closure holds it
                ups[i] = self.retry_oom(|vm| vm.heap.try_new_upvalue(UpvalState::Closed(cur)))?;
            }
        }
        let nc = self.try_closure_from_proto(proto, ups)?;
        self.set_r(base, inst.a(), Value::Closure(nc));
        // 5.1 code puts a closure straight into the variable it is assigned
        // to, which may lie below other live registers: the whole frame stays
        let live = if self.version() <= LuaVersion::Lua51 {
            (cl.proto.max_stack as u32).max(inst.a() + 1)
        } else {
            inst.a() + 1
        };
        self.maybe_collect_garbage(base + live);
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
        let uv = self.retry_oom(|vm| {
            vm.heap
                .try_new_upvalue(UpvalState::Closed(Value::Table(env)))
        })?;
        // SAFETY: `cl` is a live closure (see `Gc`), and no reference into
        // it is held across this write
        unsafe { cl.as_mut() }.upvals_mut()[idx] = uv;
        // a cell born during propagation is black: its value needs the barrier
        self.barrier_forward_upvalue(uv, Value::Table(env));
        Ok(())
    }
}
