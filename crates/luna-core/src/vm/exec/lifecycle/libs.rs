//! Opening the standard libraries on a new state.

use super::*;

impl Vm {
    /// The standalone interpreter's libraries: first what `lua.c` allocates
    /// to call its main function (5.1's `lua_cpcall` makes a C closure for
    /// it, later versions call it one level down), then
    /// [`Vm::open_all_libs`].
    #[doc(hidden)]
    pub fn open_lua_c_libs(&mut self) {
        if self.version == crate::version::LuaVersion::Lua51 {
            let size = self.heap.sizes.c_closure(0);
            let kind = crate::runtime::heap::PucKind::CClosure;
            self.heap.acct_obj(kind, size);
        } else {
            // `pmain`'s
            self.extend_ci_to(1);
        }
        self.open_all_libs();
    }

    /// Open the entire 5.5 standard library on a `new_minimal`-built Vm.
    /// `Vm::new` calls this; sandboxed embedders open libraries one at a
    /// time instead (`open_base`, `open_math`, `open_table`, …).
    pub fn open_all_libs(&mut self) {
        // what PUC's `luaL_openlibs` allocates is counted from its record,
        // after luna builds its own libraries without counting
        self.heap.puc.silent += 1;
        self.open_base();
        self.open_math();
        self.open_table();
        self.open_string();
        self.open_utf8();
        self.open_os_io();
        self.open_debug();
        self.open_coroutine();
        // PUC 5.2 introduced `bit32`; 5.3 retired it in the manual BUT
        // the stock 5.3 build ships -DLUA_COMPAT_5_2, which keeps the
        // library loaded. The diff ground truth is the default build
        // (stock lua5.3), so expose it under 5.2 AND
        // 5.3; 5.4 dropped the compat default for real.
        if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
            self.open_bit32();
        }
        // last, so `package.loaded` lists every library opened before it
        self.open_package();
        self.heap.puc.silent -= 1;
        if self.heap.acct_replay_openlibs().is_err() {
            crate::runtime::mem::oom_abort(std::alloc::Layout::new::<u8>());
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
                    // luna's own default Lua path stands for PUC's
                    if name == b"path"
                        && let Some(&puc) =
                            self.heap.puc.kept_long.iter().find(|b| b.ends_with(b"?.lua") || b.windows(5).any(|w| w == b"?.lua"))
                        && puc != s.as_bytes()
                    {
                        self.heap.acct_stand_in(s, puc);
                    }
                }
            }
        }
        self.heap.puc.kept_long = Vec::new();
        // each library opens in a call one level down, whose `CallInfo` is
        // in the record
        if self.version != LuaVersion::Lua51 {
            self.g.ci_alloc += 1;
        }
    }

    /// Install the base library (`print`, `type`, `pairs`, `tostring`,
    /// `pcall`, `error`, `assert`, `select`, `setmetatable`, `getmetatable`,
    /// `rawequal`, `rawget`, `rawset`, `rawlen`, `next`, `tonumber`,
    /// `collectgarbage`, `warn` on 5.4+, `_VERSION`, `_G`, plus 5.1's
    /// retired globals `unpack`, `loadstring`, `setfenv`, `getfenv`,
    /// `newproxy`, `gcinfo` when version == 5.1). Safe to call at most
    /// once per Vm.
    pub fn open_base(&mut self) {
        self.open_lib(crate::vm::builtins::try_open_base);
    }

    /// Install the `math` standard library.
    pub fn open_math(&mut self) {
        self.open_lib(crate::vm::lib_math::try_open_math);
    }

    /// Install the `table` standard library.
    pub fn open_table(&mut self) {
        self.open_lib(crate::vm::lib_table::try_open_table);
    }

    /// Install the `string` standard library (and the shared string metatable).
    pub fn open_string(&mut self) {
        self.open_lib(crate::vm::lib_string::try_open_string);
    }

    /// Install the `utf8` standard library (5.3+).
    pub fn open_utf8(&mut self) {
        self.open_lib(crate::vm::lib_utf8::try_open_utf8);
    }

    /// `os` and `io` are merged because file userdata shares state with both
    /// (`io.tmpname` and `os.tmpname` are the same function, `io.popen`
    /// wraps `os.execute`'s shell).
    pub fn open_os_io(&mut self) {
        self.open_lib(crate::vm::lib_os_io::try_open_os_io);
    }

    /// Install the `debug` standard library (introspection / hooks). Off by
    /// default for sandbox embedders.
    pub fn open_debug(&mut self) {
        self.open_lib(crate::vm::lib_debug::try_open_debug);
    }

    /// Install the `coroutine` standard library.
    pub fn open_coroutine(&mut self) {
        self.open_lib(crate::vm::lib_coroutine::try_open_coroutine);
    }

    /// `package` plus the 5.1-only `module` and `package.seeall` aliases.
    pub fn open_package(&mut self) {
        self.open_lib(crate::vm::lib_package::try_open_package);
    }

    /// 5.2-only `bit32` library (5.3+ retired in favour of native bitwise
    /// ops on 64-bit integers).
    pub fn open_bit32(&mut self) {
        self.open_lib(crate::vm::lib_bit32::try_open_bit32);
    }

    /// From 5.2 on, library functions are PUC light C functions, which are
    /// not collectable: luna keeps them off the swept list and frees them
    /// with the Vm, so a weak table never drops one. 5.1 collects them.
    /// A memory error here aborts, as PUC's `luaL_openlibs` does outside a
    /// protected call.
    pub(crate) fn open_lib(&mut self, open: fn(&mut Vm) -> Result<(), LuaError>) {
        crate::vm::lib_mem::opened(self.try_open_lib(open));
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
