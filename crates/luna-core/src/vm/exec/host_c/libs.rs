//! Opening one standard library at a time, for the C API's `luaopen_*`.
//!
//! From 5.2 on a `luaopen_*` function only makes its library and returns
//! it: the global and `package.loaded` entries are `luaL_requiref`'s job.
//! 5.1's register the library themselves (`luaL_register`): into the
//! table `package.loaded` or the global of that name already holds, if
//! any, else into a new one stored in both.

use super::*;
use crate::vm::{
    builtins, lib_bit32, lib_coroutine, lib_debug, lib_io, lib_math, lib_os_io, lib_package,
    lib_string, lib_table, lib_utf8,
};

/// The opener of library `name` (its global name; `_G` for the base
/// library).
fn opener(name: &str) -> fn(&mut Vm) {
    match name {
        "_G" => builtins::open_base,
        "package" => lib_package::open_package_bare,
        "coroutine" => lib_coroutine::open_coroutine,
        "table" => lib_table::open_table,
        "io" => lib_io::open_io,
        "os" => lib_os_io::open_os,
        "string" => lib_string::open_string,
        "bit32" => lib_bit32::open_bit32,
        "math" => lib_math::open_math,
        "utf8" => lib_utf8::open_utf8,
        "debug" => lib_debug::open_debug,
        _ => unreachable!("the C API names a standard library"),
    }
}

/// Why [`Vm::host_open_lib`] failed.
#[doc(hidden)]
pub enum HostOpenError {
    /// 5.1's "name conflict for module": the global of this name holds a
    /// value that is not a table
    Conflict(String),
    /// an error raised while opening (a memory error)
    Error(LuaError),
}

impl From<LuaError> for HostOpenError {
    fn from(e: LuaError) -> HostOpenError {
        HostOpenError::Error(e)
    }
}

#[doc(hidden)]
impl Vm {
    /// Open the standard library `name` (`_G` for the base library) as the
    /// dialect's `luaopen_*` does, and return what that function returns.
    pub fn host_open_lib(&mut self, name: &str) -> Result<Vec<Value>, HostOpenError> {
        let g = self.globals;
        let key = self.str_value(name.as_bytes())?;
        if name == "_G" {
            self.open_lib(builtins::open_base);
            self.open_lib(lib_os_io::open_file_loaders);
            if self.version != LuaVersion::Lua51 {
                return Ok(vec![Value::Table(g)]);
            }
            // 5.1's base library registers itself as `_G`, then the
            // coroutine library, and returns both
            let loaded = self.host_loaded()?;
            self.lib_raw_set(loaded, key, Value::Table(g))?;
            let co = self.host_open_lib("coroutine")?;
            return Ok(vec![Value::Table(g), co[0]]);
        }
        let prev = g.get(key);
        if self.version != LuaVersion::Lua51 {
            self.open_lib(opener(name));
            let fresh = g.get(key);
            self.lib_raw_set(g, key, prev)?;
            return Ok(vec![fresh]);
        }
        let loaded = self.host_loaded()?;
        let entry = loaded.get(key);
        self.open_lib(opener(name));
        let fresh = g.get(key);
        self.lib_raw_set(g, key, prev)?;
        let Value::Table(fresh_t) = fresh else {
            unreachable!("a library is a table");
        };
        let target = match (entry, prev) {
            (Value::Table(t), _) | (Value::Nil, Value::Table(t)) => {
                self.host_merge(fresh_t, t)?;
                t
            }
            (_, Value::Nil) => {
                self.lib_raw_set(g, key, fresh)?;
                fresh_t
            }
            _ => return Err(HostOpenError::Conflict(name.to_string())),
        };
        self.lib_raw_set(loaded, key, Value::Table(target))?;
        if name == "string"
            && !target.ptr_eq(fresh_t)
            && let Some(mt) = self.type_mt[3]
        {
            let k = self.str_value(b"__index")?;
            self.lib_raw_set(mt, k, Value::Table(target))?;
        }
        Ok(vec![Value::Table(target)])
    }

    /// The registry's `_LOADED` table, made on first use.
    fn host_loaded(&mut self) -> Result<Gc<Table>, LuaError> {
        let reg = self.host_registry();
        let k = self.str_value(b"_LOADED")?;
        if let Value::Table(t) = reg.get(k) {
            return Ok(t);
        }
        let t = self.alloc_table()?;
        self.lib_raw_set(reg, k, Value::Table(t))?;
        Ok(t)
    }

    /// Copy every field of `from` into `to`, as 5.1's `luaL_register` fills
    /// a library table that already exists.
    fn host_merge(&mut self, from: Gc<Table>, to: Gc<Table>) -> Result<(), LuaError> {
        let mut k = Value::Nil;
        while let Ok(Some((nk, v))) = from.next(k) {
            self.lib_raw_set(to, nk, v)?;
            k = nk;
        }
        Ok(())
    }

    fn lib_raw_set(&mut self, t: Gc<Table>, k: Value, v: Value) -> Result<(), LuaError> {
        // SAFETY: `t` is reachable from the Vm's roots (the globals, the
        // registry, or a library table held by the caller); each borrow
        // covers one store, which does not collect
        let r = self.table_op(|vm| unsafe { t.as_mut() }.set(&mut vm.heap, k, v));
        self.heap.barrier_back(t);
        r.map_err(|e| self.table_error(e))
    }
}
