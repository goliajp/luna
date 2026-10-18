//! Opening the standard libraries on a new state.

use super::*;

#[path = "libs/midcycle.rs"]
mod midcycle;
#[path = "registry.rs"]
mod registry;
#[path = "libs/reopen51.rs"]
mod reopen51;
#[path = "libs/require.rs"]
mod require;
#[path = "libs/stand_ins.rs"]
mod stand_ins;
pub(crate) use registry::{CLIBS_KEY, HOOK_KEY, light_key};

impl Vm {
    /// The standalone interpreter's libraries: first what `lua.c` allocates
    /// to call its main function (5.1's `lua_cpcall` makes a C closure for
    /// it, later versions call it one level down), then
    /// [`Vm::open_all_libs`].
    #[doc(hidden)]
    pub fn open_lua_c_libs(&mut self) -> Result<(), LuaError> {
        if self.version == crate::version::LuaVersion::Lua51 {
            let size = self.heap.sizes.c_closure(0);
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
        self.heap.puc.opening = crate::runtime::puc_libs::Opening::LuaC;
        let opened = self.open_all_libs();
        self.heap.puc.opening = crate::runtime::puc_libs::Opening::Running;
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
        if self.heap.puc.libs_opened {
            return self.reopen_all_libs();
        }
        // what PUC's `luaL_openlibs` allocates is counted from its record,
        // after luna builds its own libraries without counting
        self.heap.puc.silent += 1;
        let r = self.open_fresh_libs();
        self.heap.puc.silent -= 1;
        self.heap.puc.libs_opened = true;
        r?;
        if let Some(s) = self.heap.acct_replay_openlibs(self.stack_size)? {
            self.set_stack_size(s);
        }
        // the search paths `package` keeps are PUC's strings when they
        // have PUC's bytes
        if let Some(pkg) = self.heap.find_interned(b"package")
            && let Value::Table(p) = self.globals.get(Value::Str(pkg))
        {
            for name in [&b"path"[..], b"cpath"] {
                if let Some(k) = self.heap.find_interned(name)
                    && let Value::Str(s) = p.get(Value::Str(k))
                {
                    self.heap.acct_adopt_long(s);
                    // luna's own default Lua path stands for PUC's, as long
                    // as the package table that holds it
                    if name == b"path"
                        && let Some(&puc) = self.heap.puc.kept_long.iter().find(|(b, _)| {
                            b.ends_with(b"?.lua") || b.windows(5).any(|w| w == b"?.lua")
                        })
                        && puc.0 != s.as_bytes()
                        && crate::runtime::heap::is_uncounted(s.as_ptr() as *mut _)
                    {
                        self.heap.acct_stand_on(p.header(), puc.1, false);
                    }
                }
            }
        }
        self.heap.puc.kept_long = Vec::new();
        self.lib_stand_ins();
        // each library opens in a call one level down, whose `CallInfo` is
        // in the record
        if self.version != LuaVersion::Lua51 {
            self.g.ci_alloc += 1;
        }
        // where the collector stands after an opening that stepped: as the
        // record says, in the pause or mid-cycle; it restarts otherwise
        let opening = self.heap.libs_opening();
        let running = opening != crate::runtime::puc_libs::Opening::Stopped;
        if !self.heap.acct_libs_pace() && running {
            let rec = crate::runtime::puc_libs::midcycle(self.heap.table_dialect, opening);
            if !rec.is_some_and(|r| self.libs_midcycle(r)) {
                self.heap.gc_set_stopped(false);
            }
        }
        Ok(())
    }

    /// Every library on a Vm that has none yet, and the registry entries
    /// PUC's `luaL_openlibs` leaves.
    fn open_fresh_libs(&mut self) -> Result<(), LuaError> {
        // in PUC's order, as `luaL_requiref` opens each: where each key
        // lands in the tables they fill (and so the order `next` gives)
        // depends on it; 5.5 makes the preload table first
        let v = self.version;
        if v == LuaVersion::Lua55 {
            self.host_preload()?;
        }
        let loaded = self.host_loaded()?;
        for &name in require::openlibs_order(v) {
            self.open_fresh_lib(name, loaded)?;
            // 5.1's base library opens the coroutine library too
            if v == LuaVersion::Lua51 && name == "_G" {
                self.open_fresh_lib("coroutine", loaded)?;
            }
        }
        self.puc_registry()?;
        // `R38_LAYOUT`: how many keys of the globals and of each library
        // table a collision moved off their main position (the tools check
        // that a hash seed makes keys collide)
        #[cfg(feature = "gc-trace")]
        if std::env::var_os("R38_LAYOUT").is_some() {
            let mut k = Value::Nil;
            eprintln!("L _G {}", self.globals.keys_off_main());
            while let Ok(Some((nk, v))) = self.globals.next(k) {
                if let (Value::Str(s), Value::Table(t)) = (nk, v) {
                    eprintln!(
                        "L {} {}",
                        String::from_utf8_lossy(s.as_bytes()),
                        t.keys_off_main()
                    );
                }
                k = nk;
            }
        }
        Ok(())
    }

    /// Library `name` opened on a state that has none, as
    /// `luaL_requiref` opens it there: the opener sets the global, and
    /// `package.loaded` gets the library.
    fn open_fresh_lib(&mut self, name: &str, loaded: Gc<Table>) -> Result<(), LuaError> {
        self.try_open_lib(crate::vm::exec::host_c::lib_opener(name))?;
        let g = self.globals;
        let key = self.str_value(name.as_bytes())?;
        let lib = if name == "_G" {
            Value::Table(g)
        } else {
            g.get(key)
        };
        self.lib_raw_set(loaded, key, lib)
    }

    /// Install the base library (`print`, `type`, `pairs`, `tostring`,
    /// `pcall`, `error`, `assert`, `select`, `setmetatable`, `getmetatable`,
    /// `rawequal`, `rawget`, `rawset`, `rawlen`, `next`, `tonumber`,
    /// `collectgarbage`, `warn` on 5.4+, `_VERSION`, `_G`, plus 5.1's
    /// retired globals `unpack`, `loadstring`, `setfenv`, `getfenv`,
    /// `newproxy`, `gcinfo` when version == 5.1). Safe to call at most
    /// once per Vm.
    pub fn open_base(&mut self) -> Result<(), LuaError> {
        self.require_lib("_G", Some(crate::vm::builtins::try_open_base))
    }

    /// Install the `math` standard library.
    pub fn open_math(&mut self) -> Result<(), LuaError> {
        self.require_lib("math", Some(crate::vm::lib_math::try_open_math))
    }

    /// Install the `table` standard library.
    pub fn open_table(&mut self) -> Result<(), LuaError> {
        self.require_lib("table", Some(crate::vm::lib_table::try_open_table))
    }

    /// Install the `string` standard library (and the shared string metatable).
    pub fn open_string(&mut self) -> Result<(), LuaError> {
        self.require_lib("string", Some(crate::vm::lib_string::try_open_string))
    }

    /// Install the `utf8` standard library (5.3+).
    pub fn open_utf8(&mut self) -> Result<(), LuaError> {
        self.require_lib("utf8", Some(crate::vm::lib_utf8::try_open_utf8))
    }

    /// `os` and `io` are merged because file userdata shares state with both
    /// (`io.tmpname` and `os.tmpname` are the same function, `io.popen`
    /// wraps `os.execute`'s shell).
    pub fn open_os_io(&mut self) -> Result<(), LuaError> {
        self.require_lib("io", Some(crate::vm::lib_io::try_open_io))?;
        self.require_lib("os", Some(crate::vm::lib_os_io::try_open_os))?;
        self.open_lib(crate::vm::lib_os_io::try_open_file_loaders)
    }

    /// Install the `debug` standard library (introspection / hooks). Off by
    /// default for sandbox embedders.
    pub fn open_debug(&mut self) -> Result<(), LuaError> {
        self.require_lib("debug", Some(crate::vm::lib_debug::try_open_debug))
    }

    /// Install the `coroutine` standard library.
    pub fn open_coroutine(&mut self) -> Result<(), LuaError> {
        self.require_lib(
            "coroutine",
            Some(crate::vm::lib_coroutine::try_open_coroutine),
        )
    }

    /// `package` plus the 5.1-only `module` and `package.seeall` aliases.
    pub fn open_package(&mut self) -> Result<(), LuaError> {
        self.require_lib(
            "package",
            Some(crate::vm::lib_package::try_open_package_bare),
        )
    }

    /// 5.2-only `bit32` library (5.3+ retired in favour of native bitwise
    /// ops on 64-bit integers).
    pub fn open_bit32(&mut self) -> Result<(), LuaError> {
        self.require_lib("bit32", Some(crate::vm::lib_bit32::try_open_bit32))
    }

    /// From 5.2 on, library functions are PUC light C functions, which are
    /// not collectable: luna keeps them off the swept list and frees them
    /// with the Vm, so a weak table never drops one. 5.1 collects them.
    pub(crate) fn open_lib(
        &mut self,
        open: fn(&mut Vm) -> Result<(), LuaError>,
    ) -> Result<(), LuaError> {
        self.try_open_lib(open)
    }

    /// [`Vm::open_lib`] with an opener that raises a memory error.
    pub(crate) fn try_open_lib(
        &mut self,
        open: fn(&mut Vm) -> Result<(), LuaError>,
    ) -> Result<(), LuaError> {
        self.heap.fix_natives = self.version >= LuaVersion::Lua52;
        let r = open(self);
        self.heap.fix_natives = false;
        r
    }
}
