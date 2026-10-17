//! Building a `Vm`, opening the standard libraries, installing a JIT
//! backend, and tearing the state down.

use super::*;
use crate::runtime::mem::LVec;
use crate::vm::exec::state::CallGuards;

mod libs;
pub(crate) use libs::{CLIBS_KEY, HOOK_KEY, light_key};

impl Vm {
    /// The host runs its calls from inside a C function of its own that
    /// holds `frames` frames and `slots` stack slots below everything else,
    /// as `lua.c`'s `pmain` does: `lua_cpcall` (5.1) or `lua_pcall` with
    /// two arguments. Call once, before anything runs: the same depth then
    /// ends a recursion in both.
    #[doc(hidden)]
    pub fn host_entry_layout(&mut self, frames: u32, slots: u32) {
        self.g.host_frames += frames;
        for _ in 0..slots {
            self.stack.push_or_abort(Value::Nil);
        }
        self.top = self.stack.len() as u32;
        self.g.host_ci_top = self.top + stack_size::MIN_STACK;
    }

    /// `lua_close` from inside a running script (`os.exit(code, true)`):
    /// close the main thread's pending to-be-closed variables, then run every
    /// finalizer. Both run protected, so their errors are dropped as PUC's
    /// `close_state` drops them. Inside a coroutine the main thread's stack is
    /// parked, and only the finalizers run.
    pub(crate) fn close_state(&mut self) {
        if self.current.is_none() {
            let _ = self.close_slots(0, None);
        }
        self.run_finalizers_closing();
    }

    /// Build the raw `Vm` struct without main coroutine / RNG seed / library
    /// setup. Private helper shared by `Vm::new` and `Vm::new_minimal`; the
    /// caller is responsible for the rest of the bring-up.
    pub(super) fn new_inner(version: LuaVersion, heap: Heap) -> Option<Vm> {
        let mut heap = heap;
        heap.set_version(version).ok()?;
        // luna's own objects PUC made at other points, or not at all
        heap.puc.silent += 1;
        let globals = heap.try_new_table().ok()?;
        let mut names = [None; MM_NAMES.len()];
        for (slot, name) in names.iter_mut().zip(MM_NAMES) {
            *slot = Some(heap.try_intern(name.as_bytes()).ok()?);
        }
        let mm_names = names.map(|n| n.expect("every name was made above"));

        let mem = heap.mem();
        let mem_owner = heap.mem_owner();
        Some(Vm {
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
            compile_scratch: crate::compiler::CompileScratch::new(mem),
            g: CallGuards {
                nccalls: 0,
                stale_frames: 0,
                frame_size: if version == LuaVersion::Lua51 {
                    BASIC_FRAME_SIZE_51
                } else {
                    u32::MAX
                },
                stack_last: fast_stack_last(version, stack_size::initial(version)),
                frames_native: 0,
                host_frames: 0,
                host_ci_top: 0,
                meta_conts: 0,
                ci_alloc: [0, u32::MAX][usize::from(version == LuaVersion::Lua51)],
                host_pcalls: 0,
            },
            nny: 0,
            msgh_depth: 0,
            terminating: None,
            rng: [0; 4],
            started: std::time::Instant::now(),
            version,
            frame_cap: if version == LuaVersion::Lua51 {
                MAX_CALLS_51
            } else {
                u32::MAX
            },
            c_overflow_err: None,
            overflow_top: None,
            raise_from: None,
            varinfo_pushed: false,
            stack_size: stack_size::initial(version),
            closing_err: None,
            current: None,
            main_ctx: None,
            yielding: None,
            native_nresults: -1,
            main_coro: None,
            // PUC's `lua_newstate` starts incremental; lua.c switches to
            // generational (`gc_enter_generational`)
            gc_mode: "incremental",
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
            limited: false,
            bytecode_loading: true,
            puc_bytecode_loading: false,
            loader_input_budget: Vm::DEFAULT_LOADER_INPUT_BUDGET,
            registry: None,
            file_mt: None,
            io_input: None,
            io_output: None,
            io_stdin: None,
            ignore_env: false,
            crt_text: false,
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
            handler_bypass: None,
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
            native_window_panic: 0,
            // JIT state lives in the `JitState` sidecar; the `luna` crate's
            // constructors swap in `CraneliftBackend` for callers that want it
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
            name_cache: Default::default(),
            lib_roots: Vec::new(),
            buffer_boxes: Vec::new(),
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
            yield_ci_top: 0,
            // Pending async-native state, filled only by the dispatcher when
            // an async-marked native is invoked under async_mode.
            pending_async_native_fut: None,
            pending_async_native_ctx: None,
            jit_owner_id: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            retired_jit_storage: Vec::new(),
            buf_spare: LVec::new(mem),
            _mem: mem_owner,
            state_complete: false,
        })
    }

    /// Build a fully-loaded Vm — the default for embedders that want PUC's
    /// standard library surface. Equivalent to `Vm::new_minimal(version)`
    /// followed by `vm.open_all_libs()`; running out of memory there ends
    /// the process, as PUC's `luaL_openlibs` outside a protected call.
    pub fn new(version: LuaVersion) -> Vm {
        let mut vm = Vm::new_minimal(version);
        crate::vm::lib_mem::opened(vm.open_all_libs());
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

    /// [`Vm::new_minimal_with_mem`], or `None` when the allocation context
    /// cannot give the Vm the memory it starts with (PUC `lua_newstate`
    /// returns NULL then); what was allocated is freed.
    pub fn try_new_minimal_with_mem(
        version: LuaVersion,
        mem: crate::runtime::mem::MemOwner,
    ) -> Option<Vm> {
        let heap = Heap::try_with_mem(mem, crate::runtime::heap::fresh_seed())?;
        Vm::try_new_minimal_on(version, heap)
    }

    /// [`Vm::new`] whose memory comes from `mem` (see
    /// [`Vm::new_minimal_with_mem`]).
    pub fn new_with_mem(version: LuaVersion, mem: crate::runtime::mem::MemOwner) -> Vm {
        let mut vm = Vm::new_minimal_with_mem(version, mem);
        crate::vm::lib_mem::opened(vm.open_all_libs());
        vm
    }

    /// Bytes the Vm has allocated and not freed, as its allocation
    /// context counts them: `None` on the system allocator, which is not
    /// counted.
    pub fn memory_in_use(&self) -> Option<usize> {
        self.heap.mem().ctx().in_use()
    }

    fn new_minimal_on(version: LuaVersion, heap: Heap) -> Vm {
        Vm::try_new_minimal_on(version, heap)
            .unwrap_or_else(|| crate::runtime::mem::oom_abort(std::alloc::Layout::new::<Vm>()))
    }

    fn try_new_minimal_on(version: LuaVersion, heap: Heap) -> Option<Vm> {
        let mut vm = Vm::new_inner(version, heap)?;
        let mc = vm.heap.try_new_coro(Value::Nil, vm.globals).ok()?;
        // SAFETY: `mc` was allocated on the line above and is held only by this local; the borrow covers one field store
        unsafe { mc.as_mut() }.status = CoroStatus::Running;
        vm.main_coro = Some(mc);
        let (a, b) = vm.rng_auto_seed();
        vm.rng_seed(a as u64, b as u64);
        vm.buf_spare
            .reserve_exact(Vm::lual_buffersize(version))
            .ok()?;
        vm.state_complete = true;
        vm.heap.puc.silent -= 1;
        vm.heap.refresh_gc_limit();
        Some(vm)
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
            self.set_jit_flag(true);
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
}

impl Drop for Vm {
    fn drop(&mut self) {
        // state close: run `__gc` for every still-registered finalizable before
        // the heap frees them (PUC separatetobefnz(g,1) + callallpending). A
        // single pass — objects created by a closing finalizer are not
        // re-finalized (they go to the heap's free list directly).
        self.run_finalizers_closing();
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
