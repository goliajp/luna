//! The allocation helpers of the VM (`exec/mem.rs`) for the C API: what
//! fails for memory comes back as the memory error, after an emergency
//! collection where the dialect runs one.

use super::*;

#[doc(hidden)]
impl Vm {
    /// `bytes` as a string value.
    pub fn host_str(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        self.str_value(bytes)
    }

    /// The memory error (PUC `LUA_ERRMEM`, "not enough memory").
    pub fn host_mem_error(&mut self) -> LuaError {
        self.mem_err()
    }

    /// An emergency collection after an allocation failed, where the
    /// dialect runs one; `false` when none ran.
    pub fn host_emergency_collect(&mut self) -> bool {
        self.emergency_collect()
    }

    /// Make room for `n` more values on `co`'s C stack, collecting in an
    /// emergency when memory runs out; `false` when there is none.
    pub fn host_reserve_cstack(&mut self, co: Gc<Coro>, n: usize) -> bool {
        self.retry_oom(|_| {
            // SAFETY: `co` is a thread of this Vm the caller holds; the
            // borrow covers the reservation, which runs no Lua code
            unsafe { co.as_mut() }.host_stack.reserve(n)
        })
        .is_ok()
    }
}
