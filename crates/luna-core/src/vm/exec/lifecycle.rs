//! Building a `Vm`, opening the standard libraries, installing a JIT
//! backend, and tearing the state down.

use super::*;
use crate::runtime::mem::LVec;

impl Vm {
    /// `lua_close` from inside a running script (`os.exit(code, true)`):
    /// close the main thread's pending to-be-closed variables, then run every
    /// finalizer. Both run protected, so their errors are dropped as PUC's
    /// `close_state` drops them. Inside a coroutine the main thread's stack is
    /// parked, and only the finalizers run.
    pub(crate) fn close_state(&mut self) {
        if self.current.is_none() {
            let _ = self.close_slots(0, None);
        }
        self.heap.queue_all_finalizers();
        self.run_finalizers();
    }

    /// Build the raw `Vm` struct without main coroutine / RNG seed / library
    /// setup. Private helper shared by `Vm::new` and `Vm::new_minimal`; the
    /// caller is responsible for the rest of the bring-up.
    pub(super) fn new_inner(version: LuaVersion, heap: Heap) -> Vm {
        let mut heap = heap;
        // PUC 5.1 had no ephemeron pass — `__mode='k'` tables marked their
        // values strongly. gc.lua's "weak tables" section relies on that.
        heap.no_ephemeron = version <= LuaVersion::Lua51;
        heap.signed_zero_keys = version <= LuaVersion::Lua52;
        // PUC 5.3 needs two GC cycles to finalize a table caught in a
        // coroutine reference cycle (gc.lua :502); 5.4+ rewrote the GC and
        // finalize in a single cycle (5.4/5.5 gc.lua :544 assert exactly one).
        heap.defer_thread_cycle_finalize = version == LuaVersion::Lua53;
        let globals = heap.new_table();
        let mm_names = std::array::from_fn(|i| heap.intern(MM_NAMES[i].as_bytes()));

        let mem = heap.mem();
        let mem_owner = heap.mem_owner();
        Vm {
            heap,
            stack: LVec::new(mem),
            frames: LVec::new(mem),
            frames_top: 0,
            open_upvals: LVec::new(mem),
            tbc: LVec::new(mem),
            top: 0,
            globals,
            type_mt: [None; 7],
            mm_names,
            parse_scratch: crate::frontend::parser::ParseScratch::new(mem_owner.clone()),
            compile_scratch: Default::default(),
            c_depth: 0,
            pcall_depth: 0,
            nny: 0,
            msgh_depth: 0,
            terminating: None,
            rng: [0; 4],
            started: std::time::Instant::now(),
            version,
            closing_err: None,
            current: None,
            main_ctx: None,
            yielding: None,
            native_nresults: -1,
            main_coro: None,
            // PUC 5.4+ boots in GENERATIONAL mode (the first
            // `collectgarbage("generational")` reports "generational"
            // as the previous mode on stock lua5.4;
            // 5.5 behaves the same, probed against lua5.5). luna's
            // collector is a single incremental engine either way;
            // this field is the MODE REPORT the stdlib exposes.
            gc_mode: if version >= crate::version::LuaVersion::Lua54 {
                "generational"
            } else {
                "incremental"
            },
            gc_top: 0,
            gc_pause: 200,
            gc_stepmul: 100,
            gc_stepsize: 13,
            gc_params: crate::vm::lib_gc::GcParams::new(version),
            gc_finalizing: false,
            host_cont_hooks: None,
            host_warn: None,
            host_light: std::collections::HashMap::new(),
            warn_state: WarnState::Off,
            warn_buf: LVec::new(mem),
            warn_cont: false,
            warn_log: LVec::new(mem),
            instr_budget: None,
            bytecode_loading: true,
            puc_bytecode_loading: false,
            loader_input_budget: Vm::DEFAULT_LOADER_INPUT_BUDGET,
            registry: None,
            file_mt: None,
            io_input: None,
            io_output: None,
            io_stdin: None,
            ignore_env: false,
            hook: HookState::default(),
            in_hook: false,
            trap: true,
            pending_tailcalls: 0,
            pending_ccmt: 0,
            errored_natives: LVec::new(mem),
            msgh_floor: 0,
            msgh_running: None,
            msgh_runs: 0,
            errerr_raised: 0,
            gcmm_raised: 0,
            native_ret_hooked: false,
            tail_hook_fired: false,
            msgh_applied: None,
            keep_error_traceback: true,
            hook_ftransfer: 0,
            hook_ntransfer: 0,
            pending_tm: None,
            pending_is_hook: false,
            host_hook: None,
            hook_yield: false,
            hook_resumed: false,
            error_traceback: None,
            public_call_depth: 0,
            running_natives: LVec::new(mem),
            natives_base: 0,
            // JIT-specific state lives in the `JitState`
            // sidecar. The `luna` crate's `Vm::new_minimal_with_jit` /
            // `install_jit_backend` / `luaL_newstate` swap in
            // `CraneliftBackend` for callers that want JIT acceleration.
            jit: crate::vm::jit_state::JitState::with_null_backend(),
            // host roots ticket pool for the `Lua` facade
            host_roots: LVec::new(mem),
            // MacroLua registry. Pre-populated with
            // built-ins (`@quote` / `@unquote` / `@if` / `@gensym`)
            // when this Vm is constructed under `LuaVersion::MacroLua`.
            macro_registry: if version == LuaVersion::MacroLua {
                crate::frontend::macro_expander::MacroRegistry::with_builtins()
            } else {
                crate::frontend::macro_expander::MacroRegistry::new()
            },
            host_roots_free: LVec::new(mem),
            sort_scratch: LVec::new(mem),
            // LuaUserdata trait sugar's per-Vm
            // metatable cache. Populated lazily by register_userdata.
            userdata_metatables: std::collections::HashMap::new(),
            // Error classification metadata. Defaults to
            // Runtime; set at known sites (syntax / budget trip /
            // native error / type error).
            last_error_kind: crate::vm::error::LuaErrorKind::default(),
            last_error_source: None,
            // Async embedder fields. Defaults preserve sync behavior
            // bit-for-bit (`async_mode = false` means the budget hot loop
            // errors out instead of yielding).
            async_mode: false,
            async_waker: None,
            async_slice_size: 10_000,
            host_yield_pending: false,
            // Pending async-native state. Empty by
            // default; populated only by the dispatcher when an
            // async-marked NativeClosure is invoked under async_mode.
            pending_async_native_fut: None,
            pending_async_native_ctx: None,
            jit_owner_id: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            retired_jit_storage: Vec::new(),
            _mem: mem_owner,
        }
    }

    /// Build a fully-loaded Vm — the default for embedders that want PUC's
    /// standard library surface. Equivalent to `Vm::new_minimal(version)`
    /// followed by `vm.open_all_libs()`.
    pub fn new(version: LuaVersion) -> Vm {
        let mut vm = Vm::new_minimal(version);
        vm.open_all_libs();
        vm
    }

    /// Build a Vm with no standard libraries loaded. Embedders
    /// that want a sandbox (Redis-style scripts, in-game scripting with
    /// a curated API) call this and then `open_base` / `open_math` / etc.
    /// selectively. The Vm is otherwise fully initialized (main coroutine,
    /// RNG seed, GC) so `eval` and `call_value` are immediately usable.
    pub fn new_minimal(version: LuaVersion) -> Vm {
        Vm::new_minimal_on(version, Heap::new())
    }

    /// [`Vm::new_minimal`] with string hashing seeded by `seed` instead of
    /// a seed of its own. Where a table's string keys sit depends on the
    /// seed, and compiled traces read keys where they found them when
    /// recording, so `Vm`s that share compiled traces share a seed.
    pub fn new_minimal_with_hash_seed(version: LuaVersion, seed: u32) -> Vm {
        Vm::new_minimal_on(version, Heap::with_seed(seed))
    }

    /// [`Vm::new_minimal`] whose memory comes from `mem`: the system
    /// allocator watched by a [`crate::runtime::mem::MemoryPolicy`]
    /// ([`crate::runtime::mem::MemOwner::policy`]), or a host allocation
    /// function ([`crate::runtime::mem::MemOwner::raw`]).
    pub fn new_minimal_with_mem(version: LuaVersion, mem: crate::runtime::mem::MemOwner) -> Vm {
        Vm::new_minimal_on(version, Heap::new_on(mem))
    }

    /// [`Vm::new`] whose memory comes from `mem` (see
    /// [`Vm::new_minimal_with_mem`]).
    pub fn new_with_mem(version: LuaVersion, mem: crate::runtime::mem::MemOwner) -> Vm {
        let mut vm = Vm::new_minimal_with_mem(version, mem);
        vm.open_all_libs();
        vm
    }

    /// Bytes the Vm has allocated and not freed, as its allocation
    /// context counts them: `None` on the system allocator, which is not
    /// counted.
    pub fn memory_in_use(&self) -> Option<usize> {
        self.heap.mem().ctx().in_use()
    }

    /// What `collectgarbage("count")` and `lua_gc(LUA_GCCOUNT)` report:
    /// the allocation context's count when it keeps one, else the
    /// collector's own estimate.
    pub(crate) fn gc_count_bytes(&self) -> usize {
        self.memory_in_use().unwrap_or(self.heap.bytes())
    }

    fn new_minimal_on(version: LuaVersion, heap: Heap) -> Vm {
        let mut vm = Vm::new_inner(version, heap);
        let mc = vm.heap.new_coro(Value::Nil, vm.globals);
        // SAFETY: `mc` was allocated on the line above and is held only by this local; the borrow covers one field store
        unsafe { mc.as_mut() }.status = CoroStatus::Running;
        vm.main_coro = Some(mc);
        let (a, b) = vm.rng_auto_seed();
        vm.rng_seed(a as u64, b as u64);
        vm
    }

    /// Install a caller-supplied JIT backend. The
    /// `luna` crate uses this to swap in its `CraneliftBackend`; tests
    /// or third-party backends pass their own [`crate::jit::IntChunkCompiler`] /
    /// [`crate::jit::TraceCompiler`] implementations. A Vm starts with
    /// both JIT flags off; this turns on each flag the embedder has not
    /// set with [`Self::set_jit_enabled`] / [`Self::set_trace_jit_enabled`]
    /// (or [`Self::install_null_jit`]). Re-installing on a Vm whose
    /// closures already populated `Proto.jit: JitProtoState::Compiled`
    /// does NOT evict those cached entries — call right after
    /// construction for a clean swap.
    ///
    /// Naming: `install_jit_backend` (not `install_default_jit`)
    /// because the "default" in luna-core is `NullJitBackend`; the
    /// "default JIT" lives in the `luna` crate.
    pub fn install_jit_backend<C, T>(&mut self, chunk: C, trace: T)
    where
        C: crate::jit::IntChunkCompiler + 'static,
        T: crate::jit::TraceCompiler + 'static,
    {
        self.jit.chunk_compiler = Box::new(chunk);
        self.jit.trace_compiler = Box::new(trace);
        if !self.jit.enabled_chosen {
            self.jit.enabled = true;
        }
        if !self.jit.trace_enabled_chosen {
            self.jit.trace_enabled = true;
        }
    }

    /// Install a caller-supplied JIT
    /// storage holder. Default is [`crate::jit::NullJitStorage`];
    /// the `luna_jit` crate's `install_default_jit` pairs this with
    /// `install_jit_backend(CraneliftBackend, CraneliftBackend)` to
    /// also install a fresh `CraneliftJitStorage`. Storage holds
    /// the per-`Vm` JIT cache + handle collections.
    ///
    /// The storage it replaces is kept until the Vm drops: functions
    /// compiled through it may still be called.
    pub fn install_jit_storage<S>(&mut self, storage: S)
    where
        S: crate::jit::JitStorage + 'static,
    {
        let old = std::mem::replace(&mut self.jit.storage, Box::new(storage));
        self.retired_jit_storage.push(old);
    }

    /// Install the no-op JIT backend and switch the JIT off
    /// ([`Self::set_jit_enabled`] and [`Self::set_trace_jit_enabled`]
    /// both `false`): no hot counter ticks, no trace is recorded, and
    /// the interpreter skips the per-instruction trace lookup.
    /// Installing a real backend afterwards does not switch the JIT
    /// back on; call the two setters with `true` for that.
    ///
    /// Calling this on a Vm whose closures already populated
    /// `Proto.jit: JitProtoState::Compiled` does NOT evict those
    /// cached entries — the dispatcher will still call into them. For
    /// a truly JIT-free run, call this immediately after construction.
    pub fn install_null_jit(&mut self) {
        self.jit.chunk_compiler = Box::new(crate::jit::NullJitBackend);
        self.jit.trace_compiler = Box::new(crate::jit::NullJitBackend);
        self.set_jit_enabled(false);
        self.set_trace_jit_enabled(false);
    }

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
}

impl Drop for Vm {
    fn drop(&mut self) {
        // state close: run `__gc` for every still-registered finalizable before
        // the heap frees them (PUC separatetobefnz(g,1) + callallpending). A
        // single pass — objects created by a closing finalizer are not
        // re-finalized (they go to the heap's free list directly).
        self.heap.queue_all_finalizers();
        self.run_finalizers();
        let id = self.jit_owner_id;
        // SAFETY: the finalizers were the last Lua code this Vm runs, and
        // its functions (the only holders of entry points compiled for it)
        // go away with its heap.
        unsafe {
            self.jit.storage.release_code(id);
            for s in &mut self.retired_jit_storage {
                s.release_code(id);
            }
        }
    }
}
