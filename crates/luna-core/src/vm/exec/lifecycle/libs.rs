//! Opening the standard libraries on a new state.

use super::*;

impl Vm {
    /// Open the entire 5.5 standard library on a `new_minimal`-built Vm.
    /// `Vm::new` calls this; sandboxed embedders open libraries one at a
    /// time instead (`open_base`, `open_math`, `open_table`, …).
    pub fn open_all_libs(&mut self) {
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
    }

    /// Install the base library (`print`, `type`, `pairs`, `tostring`,
    /// `pcall`, `error`, `assert`, `select`, `setmetatable`, `getmetatable`,
    /// `rawequal`, `rawget`, `rawset`, `rawlen`, `next`, `tonumber`,
    /// `collectgarbage`, `warn` on 5.4+, `_VERSION`, `_G`, plus 5.1's
    /// retired globals `unpack`, `loadstring`, `setfenv`, `getfenv`,
    /// `newproxy`, `gcinfo` when version == 5.1). Safe to call at most
    /// once per Vm.
    pub fn open_base(&mut self) {
        self.open_lib(crate::vm::builtins::open_base);
    }

    /// Install the `math` standard library.
    pub fn open_math(&mut self) {
        self.open_lib(crate::vm::lib_math::open_math);
    }

    /// Install the `table` standard library.
    pub fn open_table(&mut self) {
        self.open_lib(crate::vm::lib_table::open_table);
    }

    /// Install the `string` standard library (and the shared string metatable).
    pub fn open_string(&mut self) {
        self.open_lib(crate::vm::lib_string::open_string);
    }

    /// Install the `utf8` standard library (5.3+).
    pub fn open_utf8(&mut self) {
        self.open_lib(crate::vm::lib_utf8::open_utf8);
    }

    /// `os` and `io` are merged because file userdata shares state with both
    /// (`io.tmpname` and `os.tmpname` are the same function, `io.popen`
    /// wraps `os.execute`'s shell).
    pub fn open_os_io(&mut self) {
        self.open_lib(crate::vm::lib_os_io::open_os_io);
    }

    /// Install the `debug` standard library (introspection / hooks). Off by
    /// default for sandbox embedders.
    pub fn open_debug(&mut self) {
        self.open_lib(crate::vm::lib_debug::open_debug);
    }

    /// Install the `coroutine` standard library.
    pub fn open_coroutine(&mut self) {
        self.open_lib(crate::vm::lib_coroutine::open_coroutine);
    }

    /// `package` plus the 5.1-only `module` and `package.seeall` aliases.
    pub fn open_package(&mut self) {
        self.open_lib(crate::vm::lib_package::open_package);
    }

    /// 5.2-only `bit32` library (5.3+ retired in favour of native bitwise
    /// ops on 64-bit integers).
    pub fn open_bit32(&mut self) {
        self.open_lib(crate::vm::lib_bit32::open_bit32);
    }

    /// From 5.2 on, library functions are PUC light C functions, which are
    /// not collectable: luna keeps them off the swept list and frees them
    /// with the Vm, so a weak table never drops one. 5.1 collects them.
    pub(crate) fn open_lib(&mut self, open: fn(&mut Vm)) {
        self.heap.fix_natives = self.version >= LuaVersion::Lua52;
        open(self);
        self.heap.fix_natives = false;
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
