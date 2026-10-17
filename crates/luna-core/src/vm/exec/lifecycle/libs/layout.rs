//! The library tables laid out as PUC's `luaL_openlibs` leaves them, from
//! its record (`puc_lib_layout.rs`): the order `next` gives over them, and
//! the order a collection traverses what they hold, are PUC's.

use super::*;

impl Vm {
    /// Lay each library table PUC's record names out again as PUC left it.
    pub(super) fn lib_layouts(&mut self) -> Result<(), LuaError> {
        let d = self.heap.table_dialect;
        for &(name, ops) in crate::runtime::puc_libs::lib_layouts(d) {
            let Some(t) = self.lib_table_named(name) else {
                continue;
            };
            // SAFETY: `t` is a library table the globals or the registry
            // hold; the borrow covers the replay, which does not collect
            let missing = unsafe { t.as_mut() }
                .replay_layout(&mut self.heap, ops)
                .map_err(|e| self.table_error(e))?;
            debug_assert!(missing.is_empty(), "{name} lacks {missing:?}");
            // `R38_LAYOUT`: how many of each table's keys left their main
            // position (`pairsgen.sh` checks that a seed makes keys collide)
            #[cfg(feature = "gc-trace")]
            if std::env::var_os("R38_LAYOUT").is_some() {
                eprintln!("L {name} {}", t.keys_off_main());
            }
        }
        Ok(())
    }

    /// The library table PUC's record calls `name` (as `libshape.lua`
    /// names them).
    fn lib_table_named(&self, name: &str) -> Option<Gc<Table>> {
        let field = |t: Gc<Table>, k: &str| {
            let k = self.heap.find_interned(k.as_bytes())?;
            match t.get(Value::Str(k)) {
                Value::Table(t) => Some(t),
                _ => None,
            }
        };
        match name {
            "_G" => Some(self.globals),
            "registry" => self.registry,
            "string_mt" => self.type_mt[3],
            "FILE*" => self.file_mt,
            _ => {
                let (parent, k) = match name.split_once('.') {
                    Some(("registry", k)) => (self.registry?, k),
                    Some(("package", k)) => (field(self.globals, "package")?, k),
                    Some(("FILE*", k)) => (self.file_mt?, k),
                    Some(_) => return None,
                    None => (self.globals, name),
                };
                field(parent, k)
            }
        }
    }
}
