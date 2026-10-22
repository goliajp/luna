//! Opening the standard libraries on a new state.

use super::*;

#[path = "registry.rs"]
mod registry;
pub(crate) use registry::{CLIBS_KEY, HOOK_KEY, light_key};

impl Vm {
    /// The standalone interpreter's libraries: first what `lua.c` allocates
    /// to call its main function (5.1's `lua_cpcall` makes a C closure for
    /// it, later versions call it one level down), then
    /// [`Vm::open_all_libs`].
    #[doc(hidden)]
    pub fn open_lua_c_libs(&mut self) -> Result<(), LuaError> {
        if self.version == crate::version::LuaVersion::Lua51 {
            let size = self.heap.puc.sizes.c_closure(0);
            let kind = crate::runtime::heap::PucKind::CClosure;
            self.heap.acct_obj(kind, size);
        } else {
            // `pmain`'s
            self.extend_ci_to(1);
            // 5.2's `lua_pcall` checks for a collection before `pmain`
            // stops the collector
            if self.version == crate::version::LuaVersion::Lua52 {
                self.heap.refresh_gc_limit();
                self.native_checkpoint()?;
            }
        }
        // lua.c stops the collector while the libraries open (5.3's leaves
        // it running), and from 5.4 on until it has made `arg`
        use crate::version::LuaVersion::{Lua53, Lua54};
        let stops = self.version != Lua53;
        if stops {
            self.heap.gc_set_stopped(true);
        }
        // lua.c's `-E`: the registry's `LUA_NOENV`, which the package
        // library reads (from 5.2 on)
        let mut opened = Ok(());
        if self.ignore_env && self.version != crate::version::LuaVersion::Lua51 {
            opened = self.set_no_env();
        }
        // the accounting trace marks where lua.c calls `luaL_openlibs`
        let opened = opened.and_then(|()| {
            self.trace_mark("openlibs");
            let r = self.open_all_libs();
            self.trace_mark("/openlibs");
            r
        });
        crate::vm::lib_mem::opened(opened);
        if stops && self.version < Lua54 {
            self.heap.gc_set_stopped(false);
        }
        Ok(())
    }

    /// Open the dialect's standard libraries, as PUC's `luaL_openlibs`.
    /// `Vm::new` calls this; sandboxed embedders open libraries one at a
    /// time instead (`open_base`, `open_math`, `open_table`, …).
    ///
    /// With the collector running, it steps while the libraries open, as
    /// in PUC; with it stopped (`host_gc` with `LUA_GCSTOP`), it does not,
    /// which is what the stand-alone interpreter does (but 5.3's).
    ///
    /// On a Vm that has opened a library before, each library opens as a
    /// second `luaL_openlibs` opens it in the dialect: 5.1 registers new
    /// functions into the library tables it finds, 5.2 makes every library
    /// anew, 5.3 on leave a library `package.loaded` holds as it is and
    /// only set its global again.
    ///
    /// # Errors
    ///
    /// A memory error, or 5.1's "name conflict for module": a library's
    /// global holds a value that is not a table and `package.loaded` has
    /// no table for it either.
    pub fn open_all_libs(&mut self) -> Result<(), LuaError> {
        self.run_openlibs()
    }

    /// `lua_pushboolean(L, 1); lua_setfield(L, LUA_REGISTRYINDEX,
    /// "LUA_NOENV")`, as lua.c does before it opens the libraries.
    fn set_no_env(&mut self) -> Result<(), LuaError> {
        let reg = self.host_registry();
        let k = self.str_value(b"LUA_NOENV")?;
        self.host_raw_set(reg, k, Value::Bool(true))
    }

    /// Install the base library (`print`, `type`, `pairs`, `tostring`,
    /// `pcall`, `error`, `assert`, `select`, `setmetatable`, `getmetatable`,
    /// `rawequal`, `rawget`, `rawset`, `rawlen`, `next`, `tonumber`,
    /// `collectgarbage`, `warn` on 5.4+, `_VERSION`, `_G`, plus 5.1's
    /// retired globals `unpack`, `loadstring`, `setfenv`, `getfenv`,
    /// `newproxy`, `gcinfo` when version == 5.1). Safe to call at most
    /// once per Vm.
    pub fn open_base(&mut self) -> Result<(), LuaError> {
        // without `dofile` and `loadfile`, which luna opens with `os` and
        // `io` (`Vm::open_os_io`)
        self.heap.puc.sandbox_base = true;
        let r = self.run_open_one("_G");
        self.heap.puc.sandbox_base = false;
        r
    }

    /// Install the `math` standard library.
    pub fn open_math(&mut self) -> Result<(), LuaError> {
        self.run_open_one("math")
    }

    /// Install the `table` standard library.
    pub fn open_table(&mut self) -> Result<(), LuaError> {
        self.run_open_one("table")
    }

    /// Install the `string` standard library (and the shared string metatable).
    pub fn open_string(&mut self) -> Result<(), LuaError> {
        self.run_open_one("string")
    }

    /// Install the `utf8` standard library (5.3+).
    pub fn open_utf8(&mut self) -> Result<(), LuaError> {
        self.run_open_one("utf8")
    }

    /// `os` and `io` are merged because file userdata shares state with both
    /// (`io.tmpname` and `os.tmpname` are the same function, `io.popen`
    /// wraps `os.execute`'s shell).
    pub fn open_os_io(&mut self) -> Result<(), LuaError> {
        self.run_open_one("io")?;
        self.run_open_one("os")?;
        crate::vm::lib_os_io::try_open_file_loaders(self)
    }

    /// Install the `debug` standard library (introspection / hooks). Off by
    /// default for sandbox embedders.
    pub fn open_debug(&mut self) -> Result<(), LuaError> {
        self.run_open_one("debug")
    }

    /// Install the `coroutine` standard library.
    pub fn open_coroutine(&mut self) -> Result<(), LuaError> {
        self.run_open_one("coroutine")
    }

    /// `package` plus the 5.1-only `module` and `package.seeall` aliases.
    pub fn open_package(&mut self) -> Result<(), LuaError> {
        self.run_open_one("package")
    }

    /// 5.2-only `bit32` library (5.3+ retired in favour of native bitwise
    /// ops on 64-bit integers).
    pub fn open_bit32(&mut self) -> Result<(), LuaError> {
        self.run_open_one("bit32")
    }
}
