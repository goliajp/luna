//! The gc-trace build's `R38_REFUSE` (see `Trace::refuse`): a growth of
//! PUC's that the instrumented PUC's allocation function refuses is
//! answered here as PUC answers it, once the operation that made it is
//! back where the VM may collect.

use super::*;

impl Vm {
    /// Answer a refused growth, if one waits.
    #[inline(always)]
    pub(crate) fn refused_growth(&mut self) -> Result<(), LuaError> {
        #[cfg(feature = "gc-trace")]
        if self.heap.trace.refused() {
            return self.answer_refusal();
        }
        Ok(())
    }

    /// [`Vm::refused_growth`] after the stack grew from `old` slots for
    /// room above `top`: 5.4 on reallocate it with emergency collections
    /// stopped, so it stays as it was and the growth is a memory error.
    #[inline(always)]
    pub(crate) fn refused_stack(&mut self, old: u32, top: u32) -> Result<(), LuaError> {
        #[cfg(feature = "gc-trace")]
        if self.heap.trace.refused() {
            if self.version >= LuaVersion::Lua54 {
                let (_, bytes) = self.heap.trace.take_refused();
                self.heap.bytes = (self.heap.bytes as i64 - bytes) as usize;
                self.set_stack_size(old);
                return Err(self.mem_err());
            }
            // `luaD_checkstack`'s `L->top` is the collection's top
            self.heap.trace.alloc_top = Some(top);
            let r = self.answer_refusal();
            self.heap.trace.alloc_top = None;
            if r.is_err() {
                self.set_stack_size(old);
            }
            return r;
        }
        let _ = (old, top);
        Ok(())
    }

    /// [`Vm::refused_growth`] after a table operation: a memory error is
    /// the table's.
    #[inline(always)]
    pub(crate) fn refused_table(&mut self) -> Result<(), crate::runtime::table::TableError> {
        self.refused_growth().map_err(|_| {
            crate::runtime::table::TableError::Memory(crate::runtime::mem::Oom(self.heap.mem()))
        })
    }

    /// PUC's `luaM_realloc_` refused: an emergency collection where the
    /// dialect makes one, without what the operation made since, then the
    /// growth again, which only an allocation function refusing every
    /// time refuses once more (a memory error, what was made since not
    /// PUC's).
    #[cfg(feature = "gc-trace")]
    #[cold]
    #[inline(never)]
    fn answer_refusal(&mut self) -> Result<(), LuaError> {
        let (lines, bytes) = self.heap.trace.take_refused();
        self.heap.bytes = (self.heap.bytes as i64 - bytes) as usize;
        // a `luaL_Buffer` box's block (5.3 on) is asked of the allocation
        // function directly, with no emergency collection
        if self.heap.trace.refused_raw {
            return Err(self.buf_error());
        }
        if self.emergency_collect() && !self.heap.trace.refuse_every {
            self.heap.bytes = (self.heap.bytes as i64 + bytes) as usize;
            self.heap.trace.put(lines);
            return Ok(());
        }
        Err(self.mem_err())
    }
}
