//! The registry as PUC's `luaL_openlibs` leaves it: the same keys, so that
//! the table has PUC's size, grows when PUC's does, and
//! `debug.getregistry` finds the same entries.

use super::*;

/// The addresses PUC keys some registry entries by (light userdata): 5.3's
/// `CLIBS`, and the debug library's hook table in 5.1 and 5.3.
pub(crate) static CLIBS_KEY: u8 = 0;
pub(crate) static HOOK_KEY: u8 = 0;

/// A light userdata keyed by the address of `k`.
pub(crate) fn light_key(k: &'static u8) -> Value {
    Value::LightUserdata(k as *const u8 as *const ())
}

impl Vm {
    /// The registry's entries after the libraries opened: `LUA_NOENV` from
    /// `lua.c`, `_LOADED`, the package library's tables, the `FILE*`
    /// metatable and the default files. The objects are luna's; PUC's were
    /// counted from its record, so this runs silent.
    pub(super) fn puc_registry(&mut self) -> Result<(), LuaError> {
        let v = self.version;
        let reg = self.host_make_registry()?;
        if self.ignore_env && v >= LuaVersion::Lua52 {
            self.registry_set(reg, "LUA_NOENV", Value::Bool(true))?;
        }
        let pkg = self.str_value(b"package")?;
        let pkg = match self.globals.get(pkg) {
            Value::Table(t) => Some(t),
            _ => None,
        };
        let field = |vm: &mut Vm, name: &[u8]| -> Result<Value, LuaError> {
            let Some(p) = pkg else {
                return Ok(Value::Nil);
            };
            let k = vm.str_value(name)?;
            Ok(p.get(k))
        };
        let loaded = field(self, b"loaded")?;
        self.registry_set(reg, "_LOADED", loaded)?;
        if v == LuaVersion::Lua51 {
            // the metatable of a loaded C library's handle, with the
            // finalizer that unloads it
            let t = self.alloc_table()?;
            let gc = self.lib_native(crate::vm::lib_package::clibs_gc)?;
            self.registry_set(t, "__gc", gc)?;
            self.registry_set(reg, "_LOADLIB", Value::Table(t))?;
        } else {
            let t = self.alloc_table()?;
            let clibs = Value::Table(t);
            if v == LuaVersion::Lua53 {
                self.raw_set(reg, light_key(&CLIBS_KEY), clibs)?;
            } else {
                self.registry_set(reg, "_CLIBS", clibs)?;
            }
            if v != LuaVersion::Lua55 {
                crate::vm::lib_package::clibs_metatable(self, t)?;
            }
            let preload = field(self, b"preload")?;
            self.registry_set(reg, "_PRELOAD", preload)?;
        }
        if let Some(mt) = self.file_mt {
            self.registry_set(reg, "FILE*", Value::Table(mt))?;
        }
        if v >= LuaVersion::Lua52 {
            if let Some(f) = self.io_input {
                self.registry_set(reg, "_IO_input", Value::Userdata(f))?;
            }
            if let Some(f) = self.io_output {
                self.registry_set(reg, "_IO_output", Value::Userdata(f))?;
            }
        }
        self.heap.barrier_back(reg);
        Ok(())
    }

    fn registry_set(&mut self, reg: Gc<Table>, name: &str, v: Value) -> Result<(), LuaError> {
        if v.is_nil() {
            return Ok(());
        }
        let k = self.str_value(name.as_bytes())?;
        self.raw_set(reg, k, v)
    }
}
