//! Opening the standard libraries on a new state.

use super::*;

#[path = "registry.rs"]
mod registry;
pub(crate) use registry::{HOOK_KEY, light_key};

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
            // 5.2's `lua_pcall` checks for a collection before `pmain`
            // stops the collector
            if self.version == crate::version::LuaVersion::Lua52 {
                self.heap.refresh_gc_limit();
                self.native_checkpoint();
            }
        }
        self.open_all_libs();
        // 5.4 on, lua.c keeps the collector stopped until it has made `arg`
        if self.version >= crate::version::LuaVersion::Lua54 {
            self.heap.gc_set_stopped(true);
        }
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
        if self.puc_registry().is_err() {
            crate::runtime::mem::oom_abort(std::alloc::Layout::new::<u8>());
        }
        self.heap.puc.silent -= 1;
        match self.heap.acct_replay_openlibs(self.stack_size) {
            Ok(Some(s)) => self.set_stack_size(s),
            Ok(None) => {}
            Err(_) => crate::runtime::mem::oom_abort(std::alloc::Layout::new::<u8>()),
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
                            self.heap.puc.kept_long.iter().find(|(b, _)| {
                                b.ends_with(b"?.lua") || b.windows(5).any(|w| w == b"?.lua")
                            })
                        && puc.0 != s.as_bytes()
                    {
                        self.heap.acct_stand_in(s, puc);
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
        let field = |vm: &Vm, t: &[u8], k: &[u8]| {
            let t = vm.heap.find_interned(t)?;
            let Value::Table(t) = vm.globals.get(Value::Str(t)) else {
                return None;
            };
            Some(t.get(Value::Str(vm.heap.find_interned(k)?)))
        };
        let stand = |vm: &mut Vm, v: Option<Value>, sizes: Vec<usize>| {
            let h = match v {
                Some(Value::Native(n)) => {
                    vm.heap.unfix_native(n);
                    n.as_ptr() as *mut _
                }
                Some(Value::Userdata(u)) => u.as_ptr() as *mut _,
                _ => return,
            };
            if crate::runtime::heap::is_uncounted(h) {
                vm.heap.puc.stand_ins.push((h, sizes));
            }
        };
        // `require` and the searchers, with `package` as their upvalue
        let req = self.heap.find_interned(b"require").map(|k| self.globals.get(Value::Str(k)));
        stand(self, req, vec![s.c_closure(1)]);
        if let Some(Value::Table(searchers)) = field(self, b"package", b"searchers") {
            for i in 1..=4 {
                stand(self, Some(searchers.get(Value::Int(i))), vec![s.c_closure(1)]);
            }
        }
        // 5.4 on, `random` and `randomseed` share the generator's state
        if self.version >= LuaVersion::Lua54 {
            let random = field(self, b"math", b"random");
            stand(self, random, vec![s.userdata(0, 32), s.c_closure(1)]);
            let seed = field(self, b"math", b"randomseed");
            stand(self, seed, vec![s.c_closure(1)]);
        }
        // 5.5 keeps its default search paths as external strings over its
        // own bytes (`LSTRFIX`): a header each
        if self.version == LuaVersion::Lua55 {
            for name in [&b"path"[..], b"cpath"] {
                let p = field(self, b"package", name);
                if let Some(Value::Str(p)) = p
                    && !self.heap.puc.stand_ins.iter().any(|s| s.0 == p.as_ptr() as *mut _)
                {
                    let h = p.as_ptr() as *mut _;
                    if crate::runtime::heap::is_uncounted(h) {
                        self.heap.puc.stand_ins.push((h, vec![s.lngstr_hdr]));
                    }
                }
            }
        }
        // the standard files, each a `LStream`
        for name in [&b"stdin"[..], b"stdout", b"stderr"] {
            let f = field(self, b"io", name);
            stand(self, f, vec![s.userdata(0, 2 * s.ptr)]);
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
