//! Raw table writes and the errors a table operation reports.

use super::*;

impl Vm {
    /// `t[key] = v` without metamethods (PUC `lua_rawset`); a read-only
    /// table refuses it before the key is looked at, as Redis's
    /// `lua_rawset` does.
    pub(crate) fn raw_set(&mut self, t: Gc<Table>, key: Value, v: Value) -> Result<(), LuaError> {
        // the barrier taken before the write tests the read-only mark too
        if !self.heap.store_barrier(t) {
            return Err(self.readonly_error());
        }
        // SAFETY: `t` is a table the caller holds (an operand of the running op or a native argument); the borrow lives for the one `set_inlined`, which touches only the heap and the table and does not collect
        match unsafe { t.as_mut() }.set_inlined(&mut self.heap, key, v) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.table_error_cold(e)),
        }
    }

    #[cold]
    #[inline(never)]
    fn table_error_cold(&mut self, e: TableError) -> LuaError {
        self.table_error(e)
    }

    /// "Attempt to modify a readonly table", positioned as `table_error`
    /// positions it.
    #[cold]
    #[inline(never)]
    pub(crate) fn readonly_error(&mut self) -> LuaError {
        self.table_error(TableError::ReadOnly)
    }

    /// `Err` with the read-only error when `t` is read-only: for the
    /// natives that change a table other than by storing a key
    /// (`setmetatable`).
    pub(crate) fn refuse_readonly(&mut self, t: Gc<Table>) -> Result<(), LuaError> {
        if t.is_readonly() {
            return Err(self.table_error(TableError::ReadOnly));
        }
        Ok(())
    }

    /// The error a refused table write raises, as the interpreter raises
    /// it: "table index is nil", "table index is NaN", "table overflow" or
    /// "Attempt to modify a readonly table", with the position of the
    /// running Lua function in front, or none when a native is running
    /// (PUC `luaG_runerror`). For an embedder that wrote through
    /// [`Table::set`] and got a [`TableError`] back.
    pub fn table_error(&mut self, e: TableError) -> LuaError {
        self.runerror(match e {
            TableError::NilIndex => "table index is nil",
            TableError::NanIndex => "table index is NaN",
            TableError::Overflow => "table overflow",
            TableError::ReadOnly => "Attempt to modify a readonly table",
            TableError::InvalidNext => "invalid key to 'next'",
        })
    }
}
