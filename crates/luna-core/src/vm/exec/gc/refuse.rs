//! The gc-trace build's `LUNA_GC_REFUSE` (see `Trace::refuse`): a growth of
//! PUC's that the instrumented PUC's allocation function refuses is
//! answered here as PUC answers it, once the operation that made it is
//! back where the VM may collect.

use super::*;

impl Vm {
    /// [`Vm::refused_stack`] when a refusal waits.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn answer_refused_stack(
        &mut self,
        old: u32,
        top: u32,
    ) -> Result<(), LuaError> {
        if self.version >= LuaVersion::Lua54 {
            let (_, bytes) = self.heap.hooks.trace.take_refused();
            self.heap.adjust_count(-bytes);
            self.set_stack_size(old);
            return Err(self.mem_err());
        }
        // `luaD_checkstack`'s `L->top` is the collection's top
        self.heap.hooks.trace.alloc_top = Some(top);
        let r = self.answer_refusal();
        self.heap.hooks.trace.alloc_top = None;
        if r.is_err() {
            self.set_stack_size(old);
        }
        r
    }

    /// PUC's `luaM_realloc_` refused: an emergency collection where the
    /// dialect makes one, without what the operation made since, then the
    /// growth again, which only an allocation function refusing every
    /// time refuses once more (a memory error, what was made since not
    /// PUC's).
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn answer_refusal(&mut self) -> Result<(), LuaError> {
        let (lines, bytes) = self.heap.hooks.trace.take_refused();
        self.heap.adjust_count(-bytes);
        // a `luaL_Buffer` box's block (5.3 on) is asked of the allocation
        // function directly, with no emergency collection
        if self.heap.hooks.trace.refused_raw {
            return Err(self.buf_error());
        }
        if self.emergency_collect() && !self.heap.hooks.trace.refuse_every {
            self.heap.adjust_count(bytes);
            self.heap.hooks.trace.put(lines);
            return Ok(());
        }
        Err(self.mem_err())
    }
}
