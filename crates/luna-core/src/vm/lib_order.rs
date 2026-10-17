//! Filling a library table in the order PUC's `luaL_openlibs` fills it,
//! from PUC's record (`puc_lib_layout.rs`): where a key lands in the hash
//! part, and so the order `next` gives, depends on that order and on the
//! sizes the table was made with.

use crate::runtime::puc_libs::LibOp;
use crate::runtime::{Gc, Table, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

impl Vm {
    /// PUC's record of what `luaL_openlibs` does to its table `name`.
    fn lib_record(&self, name: &str) -> impl Iterator<Item = LibOp> + use<> {
        crate::runtime::puc_libs::lib_layout(self.heap.table_dialect, name)
    }

    /// A new library table, PUC's `name`, with the sizes PUC makes it with.
    pub(crate) fn lib_table_puc(&mut self, name: &str) -> Result<Gc<Table>, LuaError> {
        match self.lib_record(name).next() {
            Some(LibOp::R(a, h)) => self.alloc_table_sized(a as usize, h as usize),
            _ => self.alloc_table(),
        }
    }

    /// Put `entries` into `t`, PUC's library table `name`, in the order PUC
    /// puts them there.
    pub(crate) fn lib_put_all(
        &mut self,
        t: Gc<Table>,
        name: &str,
        mut entries: Vec<(&str, Value)>,
    ) -> Result<(), LuaError> {
        for op in self.lib_record(name) {
            let LibOp::K(key) = op else {
                continue;
            };
            // most keys differ in length or first byte: no call to compare
            let first = |k: &[u8]| k.len() == key.len() && k.first() == key.first();
            let Some(at) = entries
                .iter()
                .rposition(|(k, _)| first(k.as_bytes()) && k.as_bytes() == key)
            else {
                continue;
            };
            let (k, v) = entries.swap_remove(at);
            let k = self.str_value(k.as_bytes())?;
            self.lib_set_value(t, k, v)?;
        }
        debug_assert!(
            entries.is_empty(),
            "{name}: {entries:?} not in PUC's record"
        );
        for (k, v) in entries {
            let k = self.str_value(k.as_bytes())?;
            self.lib_set_value(t, k, v)?;
        }
        Ok(())
    }
}
