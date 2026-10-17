//! Opening the standard libraries on a new state.

use super::*;

#[path = "registry.rs"]
mod registry;
#[path = "libs/reopen51.rs"]
mod reopen51;
#[path = "libs/require.rs"]
mod require;
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
        crate::vm::lib_mem::opened(self.open_all_libs());
        // 5.4 on, lua.c keeps the collector stopped until it has made `arg`
        if self.version >= crate::version::LuaVersion::Lua54 {
            self.heap.gc_set_stopped(true);
        }
        Ok(())
    }

    /// Open the dialect's standard libraries, as PUC's `luaL_openlibs`.
    /// `Vm::new` calls this; sandboxed embedders open libraries one at a
    /// time instead (`open_base`, `open_math`, `open_table`, …).
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
        self.heap.acct_libs_pace();
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

    /// The library objects of luna's, made without counting, that stand
    /// for PUC's C closures with an upvalue and userdata: the record
    /// counted PUC's; a marking counts them and a sweep frees them with
    /// luna's. (5.1's C functions are all closures, counted as luna's.)
    fn lib_stand_ins(&mut self) {
        if self.version == LuaVersion::Lua51 {
            return;
        }
        let s = self.heap.sizes;
        let lib = |vm: &Vm, t: &[u8]| match vm.globals.get(Value::Str(vm.heap.find_interned(t)?)) {
            Value::Table(t) => Some(t),
            _ => None,
        };
        let (package, math, io) = (lib(self, b"package"), lib(self, b"math"), lib(self, b"io"));
        let field = |vm: &Vm, t: Option<Gc<Table>>, k: &[u8]| {
            Some(t?.get(Value::Str(vm.heap.find_interned(k)?)))
        };
        // the natives among them leave the fixed list together, in one walk,
        // and then stand for PUC's objects as the userdata do at once
        let mut natives = [(std::ptr::null_mut(), [0; 2], 0); 8];
        let mut n_natives = 0;
        let mut stand = |vm: &mut Vm, v: Option<Value>, sizes: &[usize]| {
            let h = match v {
                Some(Value::Native(n)) => {
                    let mut kept = [0; 2];
                    kept[..sizes.len()].copy_from_slice(sizes);
                    natives[n_natives] = (n.as_ptr() as *mut _, kept, sizes.len());
                    n_natives += 1;
                    return;
                }
                Some(Value::Userdata(u)) => u.as_ptr() as *mut _,
                _ => return,
            };
            if crate::runtime::heap::is_uncounted(h) {
                vm.heap.puc.stand_ins.extend(sizes.iter().map(|&s| (h, s)));
            }
        };
        // `require` and the searchers, with `package` as their upvalue
        let req = self
            .heap
            .find_interned(b"require")
            .map(|k| self.globals.get(Value::Str(k)));
        stand(self, req, &[s.c_closure(1)]);
        // 5.2's `module`, with `package` as its upvalue too
        if self.version == LuaVersion::Lua52 {
            let m = self
                .heap
                .find_interned(b"module")
                .map(|k| self.globals.get(Value::Str(k)));
            stand(self, m, &[s.c_closure(1)]);
        }
        if let Some(Value::Table(searchers)) = field(self, package, b"searchers") {
            for i in 1..=4 {
                stand(self, Some(searchers.get(Value::Int(i))), &[s.c_closure(1)]);
            }
        }
        // 5.4 on, `random` and `randomseed` share the generator's state
        if self.version >= LuaVersion::Lua54 {
            let random = field(self, math, b"random");
            stand(self, random, &[s.userdata(0, 32), s.c_closure(1)]);
            let seed = field(self, math, b"randomseed");
            stand(self, seed, &[s.c_closure(1)]);
        }
        // 5.5 keeps its default search paths as external strings over its
        // own bytes (`LSTRFIX`): a header each, as long as the package
        // table that holds them
        if self.version == LuaVersion::Lua55
            && let Some(pkg) = package
        {
            for name in [&b"path"[..], b"cpath"] {
                if let Some(Value::Str(p)) = field(self, package, name)
                    && crate::runtime::heap::is_uncounted(p.as_ptr() as *mut _)
                {
                    self.heap.acct_stand_on(pkg.header(), s.lngstr_hdr, false);
                }
            }
        }
        // the standard files, each a `LStream`
        for name in [&b"stdin"[..], b"stdout", b"stderr"] {
            let f = field(self, io, name);
            stand(self, f, &[s.userdata(0, 2 * s.ptr)]);
        }
        let natives = &natives[..n_natives];
        let mut hs = [std::ptr::null_mut(); 8];
        for (h, n) in hs.iter_mut().zip(natives) {
            *h = n.0;
        }
        self.heap.unfix_natives(&hs[..natives.len()]);
        for &(h, sizes, n) in natives {
            if crate::runtime::heap::is_uncounted(h) {
                self.heap
                    .puc
                    .stand_ins
                    .extend(sizes[..n].iter().map(|&s| (h, s)));
            }
        }
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
