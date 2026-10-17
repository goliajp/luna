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

    /// Put `entries` into `t`, PUC's library table `name`, which the
    /// libraries list in the order PUC puts them there (checked against
    /// PUC's record in debug builds, and by `lib_pairs_order` in the order
    /// `next` gives).
    pub(crate) fn lib_put_all(
        &mut self,
        t: Gc<Table>,
        name: &str,
        entries: Vec<(&str, Value)>,
    ) -> Result<(), LuaError> {
        #[cfg(debug_assertions)]
        {
            let mut rec = self.lib_record(name).filter_map(|op| match op {
                LibOp::K(k) => Some(k),
                LibOp::R(..) => None,
            });
            for (k, _) in &entries {
                assert!(
                    rec.any(|r| r == k.as_bytes()),
                    "{name}: {k} out of PUC's order, or not in its record"
                );
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = name;
        for (k, v) in entries {
            let k = self.str_value(k.as_bytes())?;
            self.lib_set_value(t, k, v)?;
        }
        Ok(())
    }
}
