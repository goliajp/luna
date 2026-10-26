//! The registry the C API sees, as `lua_newstate` makes it.

use super::*;
use crate::runtime::puc::api::PucCount;

impl Vm {
    /// PUC's registry (`LUA_REGISTRYINDEX`), made now if it was not
    /// (`Vm::host_make_registry`), ending the process if it cannot be.
    pub(crate) fn host_registry(&mut self) -> Gc<Table> {
        match self.registry {
            Some(r) => r,
            None => self.host_make_registry().unwrap_or_else(|_| {
                crate::runtime::mem::oom_abort(std::alloc::Layout::new::<Table>())
            }),
        }
    }

    /// Make the registry (PUC `init_registry`) with the entries
    /// `lua_newstate` puts in it: from 5.2 on the main thread and the
    /// globals (5.5 also `false` at index 1, for `luaL_ref`).
    pub(crate) fn host_make_registry(&mut self) -> Result<Gc<Table>, LuaError> {
        if let Some(r) = self.registry {
            return Ok(r);
        }
        // 5.1 `luaH_new(L, 0, 2)`; later `luaH_resize` to `LUA_RIDX_LAST`
        let (a, h) = match self.version {
            LuaVersion::Lua51 => (0, 2),
            LuaVersion::Lua55 => (3, 0),
            _ => (2, 0),
        };
        let reg = self.alloc_table_sized(a, h)?;
        self.registry = Some(Heap::boot_table(reg));
        let main = Value::Coro(self.host_main_thread());
        let globals = Value::Table(self.globals);
        let entries: &[(i64, Value)] = match self.version {
            LuaVersion::Lua51 => &[],
            LuaVersion::Lua55 => &[(1, Value::Bool(false)), (3, main), (2, globals)],
            _ => &[(1, main), (2, globals)],
        };
        for &(k, v) in entries {
            // SAFETY: `reg` is rooted through `self.registry`; the borrow
            // covers one store into its array part, which does not collect
            let r = unsafe { reg.as_mut() }.set_int_raw(&mut self.heap, k, v);
            debug_assert!(r.is_ok(), "a slot of the array part");
        }
        self.heap.barrier_back(reg);
        Ok(reg)
    }
}
