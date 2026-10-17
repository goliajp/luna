//! Opening a library on a state that may have it already, as PUC's
//! `luaL_openlibs` does on a second call and `luaL_requiref` does for one
//! library: 5.1 re-registers the library into the table it finds, 5.2
//! makes it anew, 5.3 on leave a loaded library as it is and only set
//! its global again.

use super::*;
use crate::vm::exec::host_c::HostOpenError;

/// The libraries `luaL_openlibs` opens, in its order (`linit.c`), by the
/// name each is registered under.
pub(super) fn openlibs_order(v: LuaVersion) -> &'static [&'static str] {
    match v {
        LuaVersion::Lua51 => &[
            "_G", "package", "table", "io", "os", "string", "math", "debug",
        ],
        LuaVersion::Lua52 => &[
            "_G",
            "package",
            "coroutine",
            "table",
            "io",
            "os",
            "string",
            "bit32",
            "math",
            "debug",
        ],
        LuaVersion::Lua53 => &[
            "_G",
            "package",
            "coroutine",
            "table",
            "io",
            "os",
            "string",
            "math",
            "utf8",
            "debug",
            "bit32",
        ],
        LuaVersion::Lua55 => &[
            "_G",
            "package",
            "coroutine",
            "debug",
            "io",
            "math",
            "os",
            "string",
            "table",
            "utf8",
        ],
        _ => &[
            "_G",
            "package",
            "coroutine",
            "table",
            "io",
            "os",
            "string",
            "math",
            "utf8",
            "debug",
        ],
    }
}

impl Vm {
    /// [`Vm::open_all_libs`] on a state that has opened a library before:
    /// each library as `luaL_openlibs` opens it again.
    pub(super) fn reopen_all_libs(&mut self) -> Result<(), LuaError> {
        for &name in openlibs_order(self.version) {
            #[cfg(feature = "gc-trace")]
            self.trace_mark(&format!(
                "lib {}",
                if name.is_empty() { "base" } else { name }
            ));
            self.require_lib(name, None)?;
        }
        #[cfg(feature = "gc-trace")]
        if self.version == LuaVersion::Lua52 {
            self.trace_mark("lib preload");
        }
        Ok(())
    }

    /// Library `name` (`_G` for the base library) as `luaL_requiref(L,
    /// name, luaopen_*, 1)` opens it (5.1: as its `luaopen_*` registers
    /// it): 5.3 on leave one `package.loaded` holds as it is.
    /// `open` is luna's opener of it, when not the one the C API uses.
    pub(crate) fn require_lib(
        &mut self,
        name: &str,
        open: Option<fn(&mut Vm) -> Result<(), LuaError>>,
    ) -> Result<(), LuaError> {
        self.heap.puc.libs_opened = true;
        if self.version == LuaVersion::Lua51 {
            return self.reopen_lib_51(name, open);
        }
        if self.version >= LuaVersion::Lua53
            && let Some(lib) = self.loaded_lib(name)?
        {
            return self.lib_set_global(name, lib);
        }
        let opened = match open {
            Some(f) => self.open_lib_as(name, f),
            None => self.host_open_lib(name),
        };
        let lib = match opened {
            Ok(vals) => vals[0],
            Err(HostOpenError::Error(e)) => return Err(e),
            Err(HostOpenError::Conflict(name)) => {
                let msg = format!("name conflict for module '{name}'");
                return Err(LuaError(self.str_value(msg.as_bytes())?));
            }
        };
        if self.version == LuaVersion::Lua51 {
            // `luaL_register` has stored it already
            return Ok(());
        }
        let loaded = self.host_loaded()?;
        let key = self.str_value(name.as_bytes())?;
        self.lib_raw_set(loaded, key, lib)?;
        self.lib_set_global(name, lib)
    }

    /// The value `package.loaded` (the registry's `_LOADED`) holds for
    /// `name`, when it counts as loaded.
    fn loaded_lib(&mut self, name: &str) -> Result<Option<Value>, LuaError> {
        let Some(reg) = self.registry else {
            return Ok(None);
        };
        let k = self.str_value(b"_LOADED")?;
        let Value::Table(loaded) = reg.get(k) else {
            return Ok(None);
        };
        let k = self.str_value(name.as_bytes())?;
        let v = loaded.get(k);
        Ok(v.truthy().then_some(v))
    }
}
