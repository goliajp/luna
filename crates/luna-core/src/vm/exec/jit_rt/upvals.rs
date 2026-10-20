//! Upvalues, table reads through `__index` links and closures, for compiled code.

use super::*;

impl Vm {
    /// Upvalue `idx` of `cl`, for compiled code running a function a trace
    /// inlined, whose head frame starts at `head_base`: `None` when the
    /// upvalue is open at a slot of the running thread at or above
    /// `head_base`, a register the trace may hold only in its own
    /// registers while the stack has an older value.
    #[doc(hidden)]
    pub fn jit_upval_below(
        &self,
        cl: Gc<LuaClosure>,
        idx: u32,
        head_base: u32,
    ) -> Option<crate::runtime::Value> {
        use crate::runtime::UpvalState;
        match cl.upvals()[idx as usize].state() {
            UpvalState::Open { slot, thread }
                if slot >= head_base && self.is_current_thread(thread) =>
            {
                None
            }
            UpvalState::Open { slot, thread } => Some(self.read_slot(slot, thread)),
            UpvalState::Closed(v) => Some(v),
        }
    }

    /// `t[key]` through table-valued `__index` links (up to four, as the
    /// interpreter's fast path follows), for compiled code: `None` when a
    /// link is a function or the chain goes on, which the interpreter has
    /// to run.
    #[doc(hidden)]
    pub fn jit_index_str_tables(
        &self,
        t: Gc<Table>,
        key: Gc<crate::runtime::string::LuaStr>,
    ) -> Option<crate::runtime::Value> {
        use crate::runtime::Value;
        let mut cur = t;
        for _ in 0..4 {
            let v = cur.get_str(key);
            if !v.is_nil() {
                return Some(v);
            }
            let Some(mt) = cur.metatable() else {
                return Some(Value::Nil);
            };
            match self.fast_tm(mt, Mm::Index) {
                Value::Nil => return Some(Value::Nil),
                Value::Table(next) => cur = next,
                _ => return None,
            }
        }
        None
    }

    /// [`Vm::find_or_create_upval`] for a helper of compiled code: no
    /// emergency collection (the compiled code may hold new objects only
    /// in registers), `None` when the upvalue cannot be made or listed.
    #[doc(hidden)]
    pub fn jit_find_or_create_upval(&mut self, slot: u32) -> Option<Gc<Upvalue>> {
        let i = match self.open_upval(slot) {
            Ok(uv) => return Some(uv),
            Err(i) => i,
        };
        let thread = self.current;
        let uv = self.heap.try_new_upvalue(UpvalState::Open { slot, thread }).ok()?;
        self.open_upvals.insert(i, (slot, uv)).ok()?;
        Some(uv)
    }

    /// [`Vm::closure_from_proto`] for a helper of compiled code: no
    /// emergency collection, `None` when the closure cannot be made.
    #[doc(hidden)]
    pub fn jit_closure_from_proto(
        &mut self,
        proto: Gc<crate::runtime::function::Proto>,
        ups: &[Gc<Upvalue>],
    ) -> Option<Gc<LuaClosure>> {
        let cached = self.version().has_closure_cache();
        if let Some(c) = proto.cache.get().filter(|c| {
            cached
                && c.upvals().len() == ups.len()
                && c.upvals()
                    .iter()
                    .zip(ups)
                    .all(|(a, b)| a.as_ptr() == b.as_ptr())
        }) {
            return Some(c);
        }
        let n = self.heap.try_new_closure(proto, ups).ok()?;
        if cached {
            // as the interpreter's `CLOSURE` does (`luaC_protobarrier`)
            self.heap.barrier_back(proto);
            proto.cache.set(Some(n));
        }
        Some(n)
    }
}
