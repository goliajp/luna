//! The `Vm` struct and the per-call context it hands to async natives.

use super::*;
use crate::runtime::mem::LVec;

/// A Lua virtual machine: one OS thread's worth of Lua state.
///
/// # Threading model
///
/// `Vm` is **`!Send + !Sync`**. The GC uses `Gc<T> = NonNull<T>` over
/// an intrusive mark-sweep heap (not `Rc<RefCell<T>>`), and the trace
/// JIT side-table uses `Rc<CompiledTrace>` — both single-threaded by
/// design. Embedders that want concurrency spawn one `Vm` per OS
/// thread (or per single-thread Tokio worker) and exchange data via
/// channels. See [`docs/threading.md`](../../docs/threading.md) for
/// canonical embedding patterns including Tokio `current_thread`,
/// `LocalSet` on multi-thread, and `Vm`-per-OS-thread + channels.
///
/// The constraint is enforced at compile time:
///
/// ```compile_fail
/// fn must_be_send<T: Send>() {}
/// must_be_send::<luna_core::Vm>(); // error[E0277]: `Vm` cannot be sent between threads safely
/// ```
///
/// A future `feature = "send"` will gate an
/// opt-in `Arc<RwLock<T>>` mode with a hard ≤8% perf regression
/// budget.
// fixed field order: the interpreter's hot paths are tuned to this layout
// (stack, frames and the call counters near the front), and letting the
// compiler reorder it after an unrelated field change costs instructions
// on every call
#[repr(C)]
pub struct Vm {
    /// The GC heap owned by this VM. Embedders normally interact via the
    /// `Vm` methods (`load` / `call_value` / `set_global` / …) rather than
    /// the heap directly.
    pub heap: Heap,
    /// Embedding cooperative budget: a per-Vm tick counter the run loop
    /// decrements once per dispatch turn; at zero it raises "instruction
    /// budget exceeded". `None` = unbounded; `Some(0)` once it ran out,
    /// until the host calls `set_instr_budget` again (see `limits.rs`).
    pub(crate) instr_budget: Option<i64>,
    /// `instr_budget` or the heap's `mem_cap` is armed, so no compiled code
    /// is entered (`jit.gate` off, no trace admitted); see `sync_limited`
    pub(crate) limited: bool,
    pub(crate) stack: LVec<Value>,
    /// the counters every call checks, together so a call touches one
    /// cache line for them
    pub(crate) g: CallGuards,
    pub(crate) frames: LVec<CallFrame>,
    /// open upvalues, sorted ascending by stack slot
    pub(super) open_upvals: LVec<(u32, Gc<Upvalue>)>,
    /// to-be-closed slots, ascending
    pub(super) tbc: LVec<u32>,
    /// the parser's vectors, kept from one `load` to the next
    pub(super) parse_scratch: crate::frontend::parser::ParseScratch,
    /// the compiler's vectors, kept from one load to the next
    pub(super) compile_scratch: crate::compiler::CompileScratch,
    pub(crate) warn_buf: LVec<u8>,
    /// In-process log of fully-emitted warnings (each entry = one flushed
    /// message, sans the "Lua warning: " prefix and trailing newline). Lets
    /// tests assert what was warned without scraping stderr.
    pub(crate) warn_log: LVec<LVec<u8>>,
    /// Name of the C native that just propagated an error (captured before
    /// the native is popped from `running_natives`). Lets a dying coroutine
    /// preserve `[C]: in function '<name>'` at the top of its traceback
    /// snapshot — PUC walks `luaG_funcnamefrompc` over a still-live ci, but
    /// luna's native frames are off-stack so we stash the name explicitly.
    pub(crate) errored_natives: LVec<crate::vm::callstack::ErroredNative>,
    /// stack of native (`Value::Native`) closures currently running on the
    /// Rust call stack. `begin_call` pushes the closure before invoking
    /// `nc.f` and pops on return. Used by `arg_error` to detect a *nested*
    /// native call (PUC `ar.name == NULL` at level 0 because the level-0
    /// caller is C, not Lua) and qualify the running function's name via
    /// `pushglobalfuncname` (e.g. `'sort'` → `'table.sort'`).
    /// Each entry also records where the native sits on the value and
    /// frame stacks, so the debug interface can place it among the Lua
    /// activations as PUC's CallInfo chain would (see `callstack`).
    pub(crate) running_natives: LVec<crate::vm::callstack::NativeAct>,
    /// JIT sidecar. Always present (never `Option`); inert
    /// when `chunk_compiler` / `trace_compiler` are
    /// [`crate::jit::NullJitBackend`]. See [`crate::vm::jit_state`].
    ///
    /// `#[doc(hidden)] pub` so the `luna` crate's
    /// `extern "C"` JIT helpers can write `vm.jit.pending_err`
    /// directly. Not part of the embedder-facing API surface.
    #[doc(hidden)]
    pub jit: crate::vm::jit_state::JitState,

    /// Host roots — a `Vec<Value>` traced as an extra GC root set.
    /// `Lua` facade handles (`LuaFunction`, `LuaTable`, `LuaRoot`) hold
    /// indices into this vector so the underlying `Gc<T>` stays alive
    /// across `eval` calls / yield boundaries. Freed slots are recycled
    /// through `host_roots_free`.
    pub(crate) host_roots: LVec<crate::vm::host_roots::HostRootSlot>,
    /// Recycled-slot index pool. `pin_host` pops the
    /// back if non-empty, else extends `host_roots`. Generation
    /// overflow at `u32::MAX` retires the slot (NOT pushed here).
    pub(crate) host_roots_free: LVec<u32>,

    /// GC-rooted scratch stack for `table.sort` (and any other
    /// builtin that needs a Rust-side `Vec<Value>` to outlive a user
    /// callback). Each entry is one in-flight working buffer; `gc_roots`
    /// extends with every contained `Value` so a `collectgarbage()`
    /// inside the comparator cannot free strings/tables snapshotted
    /// here. Nested sorts push a new buffer on entry, pop on exit
    /// (sort.lua's `load(..)(); collectgarbage()` compare callback
    /// regression).
    pub(crate) sort_scratch: LVec<LVec<Value>>,
    /// Storages [`Vm::install_jit_storage`] replaced: code compiled into
    /// them may still be referenced by this Vm's functions, so they live
    /// as long as the Vm.
    pub(super) retired_jit_storage: Vec<Box<dyn crate::jit::JitStorage>>,
    /// the main thread's saved execution context while a coroutine runs
    pub(super) main_ctx: Option<SavedCtx>,
    /// set by `coroutine.yield` to suspend the running coroutine: the yielded
    /// values plus the slot/result-count needed to finish the yielding call on
    /// the next resume. Checked by `exec` to propagate (not unwind) on yield.
    pub(super) yielding: Option<(Vec<Value>, u32, i32)>,
    /// traceback of an error nothing in its thread catches, one line per
    /// stack level, taken where it was raised (see `raise_to_handler`): what
    /// the host gets from `take_error_traceback`, and what `debug.traceback`
    /// shows of the coroutine it kills. Cleared on a catch and at host-level
    /// `call_value` entry (`public_call_depth == 0`).
    pub(crate) error_traceback: Option<Vec<Vec<u8>>>,

    /// `(source_name, line)` of the most recent error. Set by the
    /// dispatcher / lexer / parser; cleared when a new call_value
    /// enters cleanly.
    pub(crate) last_error_source: Option<(String, u32)>,
    /// VM creation time (os.clock)
    pub(super) started: std::time::Instant,
    /// the running thread's debug hook state (`debug.sethook`); per-thread,
    /// swapped with the execution context on a coroutine resume/yield
    pub(crate) hook: HookState,
    /// `collectgarbage`'s parameters as the dialect stores them; they set
    /// the three knobs above through `set_gc_pacing`.
    pub(crate) gc_params: crate::vm::lib_gc::GcParams,
    /// error object being threaded through a chain of __close handlers; a GC
    /// root for the duration (a handler may trigger collection)
    pub(super) closing_err: Option<Value>,
    /// The message handler that is running, if any. PUC's `luaG_errormsg`
    /// calls the handler with `L->errfunc` still set, so an error inside
    /// the handler (and not caught within it) calls the handler again, at
    /// the point of that error. Per thread, like `L->errfunc`.
    pub(crate) msgh_running: Option<Value>,
    /// The value the last `xpcall` handler produced for the error in
    /// flight, so the unwind that carries it to the `xpcall` does not
    /// run the handler again.
    pub(crate) msgh_applied: Option<Value>,
    /// set by a coroutine closing itself (`coroutine.close()` on the running
    /// thread): the to-be-closed handlers have already run; the thread must now
    /// terminate. `Some(None)` is a clean close, `Some(Some(e))` a handler
    /// raised `e`. Checked by `exec_with`/`resume_coro` to propagate (not
    /// unwind, so a protecting pcall cannot catch it) the termination.
    pub(super) terminating: Option<Option<Value>>,
    pub(super) globals: Gc<Table>,
    /// pre-interned metamethod event names, indexed by `Mm`
    pub(super) mm_names: [Gc<crate::runtime::LuaStr>; MM_NAMES.len()],
    /// `collectgarbage` mode name ("incremental"/"generational"). The collector
    /// itself is still stop-the-world mark-sweep; this tracks the mode so mode
    /// switches report the previous one, as PUC does.
    pub(super) gc_mode: &'static str,
    /// The C API's functions without upvalues, one per C function pointer:
    /// from 5.2 on PUC's light C functions are equal when their pointers
    /// are. GC roots.
    pub(crate) host_light: std::collections::HashMap<usize, Value>,

    /// MacroLua compile-time macro registry.
    /// Pre-populated with built-in macros (`@quote` / `@unquote` /
    /// `@if` / `@gensym`) at construction time when `version ==
    /// LuaVersion::MacroLua`; embedders register custom macros via
    /// [`Vm::define_macro`]. The expander runs once per `load()` call
    /// between lexing and parsing (only when `is_macro_lua()`).
    pub(crate) macro_registry: crate::frontend::macro_expander::MacroRegistry,

    /// Per-Vm cache of `Gc<Table>` metatables keyed
    /// by `TypeId::of::<T>()` for embedder types implementing
    /// [`crate::vm::userdata_trait::LuaUserdata`]. Populated lazily by
    /// [`Vm::register_userdata`]; metatables are pinned via
    /// [`Vm::pin_host`] at registration time so the entry's
    /// `Gc<Table>` stays live for the rest of the Vm's lifetime.
    pub(crate) userdata_metatables:
        std::collections::HashMap<std::any::TypeId, Gc<crate::runtime::table::Table>>,
    /// shared metatable for all strings (populated by the string lib)
    /// per-basic-type metatables (PUC luaT): indexed by `type_mt_slot`
    /// (0 nil, 1 boolean, 2 number, 3 string, 4 function, 5 light userdata,
    /// 6 thread); tables and full userdata carry their
    /// own. Settable via debug.setmetatable.
    pub(super) type_mt: [Option<Gc<Table>>; 7],
    /// xoshiro256** state (math.random)
    pub(super) rng: [u64; 4],
    /// the coroutine whose context is currently live in the fields above;
    /// `None` while the main thread runs
    pub(crate) current: Option<Gc<crate::runtime::Coro>>,
    /// identity object for the main thread, returned by `coroutine.running`
    /// (the main thread's context lives in the VM fields / `main_ctx`, not here)
    pub(super) main_coro: Option<Gc<Coro>>,
    /// `collectgarbage("param", name [,value])` pacing parameters. The collector
    /// is still stop-the-world, so these are stored/returned for API fidelity
    /// (PUC round-trips them via `setparam`/`getparam`). Defaults mirror PUC's
    /// `LUAI_GC*` knobs: pause=200, stepmul=100, stepsize=13.
    pub(super) gc_pause: i64,
    pub(super) gc_stepmul: i64,
    pub(super) gc_stepsize: i64,
    /// What the C API runs for a C function's continuation
    /// (`ContKind::Host`); see [`super::host_c`].
    pub(crate) host_cont_hooks: Option<super::host_c::HostContHooks>,
    /// The C API's warning function (`lua_setwarnf`), which replaces the
    /// default one; see [`super::host_c`].
    pub(crate) host_warn: Option<super::host_c::HostWarn>,
    /// Byte budget for source fed into `load` / `loadstring` / `Vm::load`.
    /// Default [`Vm::DEFAULT_LOADER_INPUT_BUDGET`] (256 MiB). When the
    /// accumulated reader output (`load(f, ...)`) or a one-shot `&[u8]`
    /// source exceeds this, the loader returns the PUC-shaped
    /// `not enough memory` error before the host allocator is asked to
    /// hold the next chunk. Defends against `heavy.lua::loadrep`-style
    /// 7 GB+ feeder loops that would otherwise SIGSEGV when `Vec::push`
    /// crosses `isize::MAX` or the host runs out of RAM.
    /// Embedders that genuinely need to load > 256 MiB sources widen the
    /// cap via [`Vm::set_loader_input_budget`].
    pub(crate) loader_input_budget: usize,
    /// PUC's `LUA_REGISTRYINDEX` table — a single Lua table the debug library
    /// exposes via `debug.getregistry`. Used to hold `_HOOKKEY` (the weak-key
    /// table PUC's `db_sethook` keys per-thread hooks under). luna stores hook
    /// state directly in `Vm.hook`/`Coro.hook`, so the entry is largely a
    /// shape stub for db.lua :328; if other registry-keyed APIs land later
    /// they can share this table.
    pub(crate) registry: Option<Gc<Table>>,
    /// the shared `FILE*` metatable for io file handles (PUC's LUA_FILEHANDLE
    /// registry entry); attached to every file userdata the io library makes
    pub(crate) file_mt: Option<Gc<Table>>,
    /// io library default input/output streams (PUC registry IO_INPUT/IO_OUTPUT)
    pub(crate) io_input: Option<Gc<crate::runtime::Userdata>>,
    pub(crate) io_output: Option<Gc<crate::runtime::Userdata>>,
    /// `io.stdin` as the io library made it, whatever the script later does
    /// to the `io` table: the stream a host's line reads share
    pub(crate) io_stdin: Option<Gc<crate::runtime::Userdata>>,
    /// Frames below this index are out of reach of the error handler of
    /// an `xpcall` (PUC `L->errfunc`): a protected call made from Rust — a
    /// finalizer, the handler itself — starts a fresh `errfunc` scope.
    pub(crate) msgh_floor: usize,
    /// How many message-handler runs have started; lets a run tell whether
    /// the error it got back was already handled by a nested run.
    pub(crate) msgh_runs: u64,
    /// How many times an error became LUA_ERRERR ("error in error
    /// handling"); a host protected call compares it before and after to
    /// report that status instead of LUA_ERRRUN.
    pub(crate) errerr_raised: u64,
    /// the "error in error handling" just raised, until it reaches the
    /// unwinder: PUC's `luaD_throw(LUA_ERRERR)` runs no message handler
    pub(crate) errerr_in_flight: Option<Value>,
    /// finalizer errors a 5.2/5.3 full collection raised (`LUA_ERRGCMM`)
    pub(crate) gcmm_raised: u64,
    /// The C API's dispatcher of C hook functions: a thread whose hook
    /// function is a light userdata has a C hook (`lua_sethook`), which
    /// this runs; see [`super::host_c`].
    pub(crate) host_hook: Option<super::host_c::HostHookFn>,
    /// Index into `running_natives` where the running thread's own natives
    /// begin; the ones below belong to the threads that resumed it.
    pub(crate) natives_base: usize,

    /// Host waker cloned by `EvalFuture::poll` before driving a slice.
    /// The dispatcher itself does not call it (the future's poll loop
    /// does `wake_by_ref` after observing `BudgetExhausted`); it is kept
    /// so async natives can wake the host directly from a helper future.
    pub(crate) async_waker: Option<std::task::Waker>,

    /// Per-poll opcode quota loaded into
    /// `instr_budget` at the start of each `EvalFuture::poll` slice.
    /// Default 10_000. Tunable via
    /// [`Vm::set_async_slice`].
    pub(crate) async_slice_size: i64,

    /// Set by the dispatcher's native-call path
    /// when an async-marked [`NativeClosure`] is invoked under
    /// `async_mode`. The Vm pauses the dispatcher (same sentinel-Err
    /// mechanism as `host_yield_pending` — see `exec_with` +
    /// `call_value_impl`), stashes the in-flight future +
    /// post-completion context here, and surfaces them to
    /// `EvalFuture::poll` via `drive_one`. Cleared by `drive_one`
    /// once the future is moved out into a
    /// `DispatchOutcome::AsyncNativeAwaiting`.
    pub(crate) pending_async_native_fut:
        Option<std::pin::Pin<Box<dyn std::future::Future<Output = Result<u32, LuaError>>>>>,

    /// Identifies this Vm to the JIT storages it compiles through
    /// ([`crate::jit::JitStorage::claim`]).
    pub(super) jit_owner_id: u64,

    /// Companion to `pending_async_native_fut`:
    /// the `(func_slot, nargs, nresults, gc_top)` quad needed to
    /// commit the future's eventual `Ok(nret)` back into the calling
    /// frame's expected result slots. Recorded by the dispatcher;
    /// consumed by [`Vm::commit_async_native_result`] after the
    /// future resolves.
    pub(crate) pending_async_native_ctx: Option<AsyncNativeCallCtx>,
    /// Shadow of `self.frames.len()`. Synced on every push/pop in the
    /// `frames_push_sync`/`frames_pop_sync` helpers (debug-asserted on
    /// use). Not consumed by readers yet; it is scaffolding for replacing
    /// `frames: Vec<CallFrame>` with a flat `[CallFrame; MAX_FRAMES]`
    /// indexed by frames_top.
    pub(super) frames_top: u32,
    /// logical stack top for multi-result sequences
    pub(crate) top: u32,
    /// number of non-yieldable C calls in flight on the running thread (PUC's
    /// `L->nny`). A library callback that runs via synchronous Rust recursion
    /// (sort comparator, gsub replacement) cannot be continued across a yield,
    /// so it bumps this for its duration; `coroutine.yield` inside hits the
    /// C-call boundary and errors. Always 0 at a suspend point (a yield can
    /// never cross such a call); a resume starts the coroutine at 0 and puts
    /// the resumer's count back after.
    pub(super) nny: u32,
    /// Nonzero while an xpcall message handler is on the Rust stack. Used so a
    /// stack-overflow that surfaces *inside* the handler is reported as PUC's
    /// "error in error handling" (LUA_ERRERR + `luaD_seterrorobj`), not the
    /// plain "stack overflow" — errors.lua :606's `checkerr("error handling",
    /// loop)` then matches. PUC tracks this via the soft-cap window
    /// `nCcalls >= MAXCCALLS/10*11`; luna's c_depth is strict, so we mark the
    /// scope explicitly.
    pub(crate) msgh_depth: u32,
    /// results expected by the in-flight native call (so `yield` knows how many
    /// values its call site wants when it suspends)
    pub(super) native_nresults: i32,
    /// the live-register boundary of the running thread for GC rooting (PUC's
    /// `L->top`): set precisely at each GC safe point so freed temporary
    /// registers above it are not rooted. Without this the collector roots the
    /// whole stack window, pinning weak-table values stranded in stale temps
    /// (e.g. closure.lua's `while x[1]` GC-detection loop).
    pub(crate) gc_top: u32,
    /// arms the next Lua frame's `tailcalls` count (PUC `ci->u.l.tailcalls`),
    /// consumed by `push_frame`. `OP_TailCall` sets it to the caller's
    /// own tailcalls + 1 before begin_call so deeply tail-recursive chains
    /// accumulate the count instead of capping at 1.
    pub(crate) pending_tailcalls: u32,
    /// nesting depth of public `call_value` entries (host vs. internal). The
    /// outermost entry (depth 0) resets per-error state (`error_traceback`);
    /// internal calls (e.g. xpcall msgh, sort callback) preserve it.
    pub(super) public_call_depth: u32,
    /// PUC `CallInfo.u2.transferinfo`: index of the first transferred value
    /// (relative to the activation's func slot) and the number transferred.
    /// Set just before firing a call/return hook, read by `getinfo("r")`.
    pub(crate) hook_ftransfer: u16,
    pub(crate) hook_ntransfer: u16,
    /// true while `__gc` finalizers are being run, so a finalizer that calls
    /// `collectgarbage` gets a no-op (PUC's non-reentrancy: lua_gc returns -1 →
    /// `collectgarbage` yields fail).
    pub(super) gc_finalizing: bool,
    /// PUC 5.4+ warning system. Lua manual §6.1 `warn`: emitted messages
    /// concatenate across continuation calls until a non-`tocont` call
    /// flushes; the default warnf recognises `@on`/`@off` control messages
    /// and starts disabled. luna's `emit_warn` mirrors the default warnf
    /// behaviour and 5.4+ `__gc` errors are routed through it (5.1–5.3
    /// keep the older raise semantics).
    pub(crate) warn_state: WarnState,
    /// the default warning function is in the middle of a message (PUC
    /// `warnfcont`)
    pub(crate) warn_cont: bool,
    // JIT-specific state lives in the `JitState` sidecar; see `self.jit`
    // below and `crate::vm::jit_state` for field docs.
    /// Bytecode-loading gate. Default `true`. Sandbox embedders should
    /// call `set_bytecode_loading(false)` so `load`/`loadstring` reject
    /// precompiled chunks (which bypass the parser's depth / opcode
    /// limits). When `false`, the loader rejects any source whose first
    /// byte is the bytecode signature `\27` ("`\27Lua`").
    pub(crate) bytecode_loading: bool,
    /// PUC bytecode-loading gate. Default `false` — PUC `.luac` files are
    /// a strictly larger trust surface than luna's own dump format
    /// (third-party toolchain bugs, malformed chunks, unknown opcode
    /// shapes). When `true`, the loader routes `\x1bLua\x{51..55}` inputs
    /// through the per-dialect PUC translators in `crate::vm::dump::puc`.
    /// Embedder toggles via `set_puc_bytecode_loading`.
    pub(crate) puc_bytecode_loading: bool,
    /// lua.c's `-E`: libraries opened from now on ignore the environment
    pub(crate) ignore_env: bool,
    /// files opened without `b` behave as in the MSVC C library's text mode
    pub(crate) crt_text: bool,
    /// true while the hook itself runs, so its own execution fires no events
    /// (PUC clears the mask for the duration)
    pub(crate) in_hook: bool,
    /// PUC `trap`: the dispatch loop head has work beyond fetching the next
    /// instruction — an instruction budget, a memory cap or an armed hook.
    /// The loop head clears it when it finds none of them; whatever may
    /// create one sets it (set spuriously, it costs one slow iteration).
    pub(crate) trap: bool,
    /// the running native fired its own return hook (a C function of the C
    /// API, whose values live on its C stack): its return skips the hook
    pub(crate) native_ret_hooked: bool,
    /// the call hook of the Lua function a 5.1–5.3 tail call is entering
    /// already ran, before the caller's frame went (see `tail_call_hook`)
    pub(crate) tail_hook_fired: bool,
    /// Whether an error nothing catches should keep its traceback: not
    /// inside a protected call made from Rust, which discards it.
    pub(crate) keep_error_traceback: bool,
    /// `true` when the next `push_frame` is the user hook function itself,
    /// so `debug.getinfo(1).namewhat` resolves to `"hook"` (PUC
    /// `CIST_HOOKED`). `run_hook` arms it before dispatching the hook.
    pub(super) pending_is_hook: bool,
    /// A C line or count hook asked to yield (`lua_yield` inside a hook);
    /// acted on once the hooks of the instruction have run.
    pub(crate) hook_yield: bool,
    /// The running thread resumed from a hook's yield (5.2+
    /// `CIST_HOOKYIELD`): the next hook check does not call the hook again.
    pub(crate) hook_resumed: bool,

    /// When `true`, `instr_budget` exhaustion in
    /// the dispatcher hot loop yields cooperatively (sets
    /// [`Vm::host_yield_pending`] + returns a sentinel `Err` walked up
    /// to `EvalFuture::poll`) instead of returning a real
    /// "instruction budget exceeded" error. Set by [`Vm::eval_async`]
    /// for the duration of the future; restored to `false` on
    /// `Poll::Ready`. The sync `Vm::eval` / `Vm::call_value` paths
    /// leave it `false` so budget exhaustion stays a real error there.
    pub(crate) async_mode: bool,

    /// Set by the dispatcher when an async-mode budget exhaustion fires;
    /// checked by `exec_with` (so the sentinel propagates without `unwind`
    /// running, mirroring `yielding.is_some()`) and by `call_value_impl`
    /// (so the call frames survive for the next poll). Cleared by
    /// `drive_one` after translating it to `DispatchOutcome::BudgetExhausted`.
    pub(crate) host_yield_pending: bool,
    /// metamethod event tag (e.g. "close") to attach to the next Lua frame
    /// pushed by `push_frame`; `close_slots` sets this before calling a
    /// `__close` handler so `debug.traceback` names it "metamethod 'close'"
    /// (PUC `CallInfo.u.l.tm`). Single-shot: `push_frame` consumes it.
    pub(super) pending_tm: Option<crate::runtime::function::FrameTm>,
    pub(super) version: LuaVersion,

    /// Classification of the most recent error raised on this Vm.
    /// Embedders read via [`Vm::error_kind`]; the dispatcher sets it
    /// at well-known sites (syntax errors, instr-budget trips, native
    /// callback errors, type errors).
    pub(crate) last_error_kind: crate::vm::error::LuaErrorKind,
    /// arms the next Lua frame's `ccmt` (its `__call` chain length), consumed
    /// by `push_frame`. `OP_TailCall` sets it to the reused activation's
    /// count; `begin_call` otherwise sets the chain it just resolved.
    pub(super) pending_ccmt: u8,
    /// The allocation context the Vm's own containers (the stack and the
    /// frame stack above) free through. Last, so it outlives them: the
    /// heap, which owns it too, is the first field to be dropped.
    pub(super) _mem: crate::runtime::mem::MemOwner,
    /// Lua frames a thread may hold before a call raises "stack
    /// overflow": PUC 5.1's `LUAI_MAXCALLS`; no count in later dialects,
    /// whose limit is the stack size. The frame array is grown as PUC
    /// 5.1 grows its `CallInfo` array (see `grow_frames`).
    pub(super) frame_cap: u32,
    /// the slot the message handler runs at for a stack overflow or a call
    /// of a value that cannot be called, which PUC raise from the top of
    /// that call (`L->top` when `luaD_growstack` or `luaG_callerror` raised)
    pub(crate) overflow_top: Option<u32>,
    /// the last runtime error a Lua frame raised named its operand, which
    /// 5.3+'s `varinfo` pushes on the stack before the message: the message
    /// handler runs one slot higher
    pub(crate) varinfo_pushed: bool,
    /// the "C stack overflow" a call was last refused with: PUC's refused
    /// call keeps its level while its error is in flight, which lets the
    /// message handler run on it one level above the limit
    pub(crate) c_overflow_err: Option<Value>,
    /// the running thread's stack has overflowed and is using the error
    /// space (PUC's stack grown to `ERRORSTACKSIZE`), until a protected
    /// call catches the error; a call that does not fit it is "error in
    /// error handling". Per-thread, saved with the coroutine context.
    pub(super) stack_extra: bool,
}

/// Call-site context an in-flight async native
/// needs preserved across the cooperative-yield boundary.
///
/// The dispatcher records this when it routes a `NativeClosure` with
/// `is_async == true` through the cooperative path; `EvalFuture::poll`
/// hands it back to [`Vm::commit_async_native_result`] once the
/// awaited future resolves so `finish_results` (and the post-call GC
/// checkpoint) can run as if the native had completed synchronously.
#[derive(Clone, Copy)]
pub(crate) struct AsyncNativeCallCtx {
    pub func_slot: u32,
    /// Recorded for parity with the sync native-call path's
    /// `native_nresults`/`gc_top` bookkeeping; reserved for hook
    /// firing + traceback shaping. Not read yet.
    #[allow(dead_code)]
    pub nargs: u32,
    pub nresults: i32,
    /// Recorded for traceback + GC-root-window checks. The resume path
    /// reads `Vm.gc_top` directly, so this is unread today; carried so a
    /// check can confirm the pre-suspend root window matches the
    /// post-resume one.
    #[allow(dead_code)]
    pub gc_top: u32,
}

/// The counters every call checks and keeps (see each field's note):
/// together so a call touches one cache line for all of them.
#[repr(C)]
pub(crate) struct CallGuards {
    /// PUC `nCcalls`: the C levels in flight on the running thread (see
    /// `MAX_C_DEPTH`): calls native code made into Lua, the pcall /
    /// xpcall, metamethod, `__pairs` and `__close` continuations above the
    /// thread's last resume, the resumes below it, and the levels refused
    /// calls hold while their message handlers run. A resume starts the
    /// coroutine from the resumer's count (`lua_resume`), so a thread's
    /// count is not saved with it.
    pub(crate) nccalls: u32,
    /// frames the running thread had when it was last resumed: PUC's
    /// `lua_resume` starts the thread from the resumer's count, so the
    /// continuations below hold no level and popping them gives none
    /// back. Per-thread, saved with the coroutine context.
    pub(crate) stale_frames: u32,
    /// the size of PUC 5.1's `CallInfo` array: doubled when the frames in
    /// use (`Vm::frames_in_use`) fill it, which is where its limit is
    /// checked (`grow_frames`). Per-thread. `u32::MAX` in later dialects.
    pub(crate) frame_size: u32,
    /// `lua_stack_limit` of the dialect
    pub(crate) lua_stack_limit: u32,
    /// calls compiled code made on the native stack below the running
    /// interpreter frames, which take no frame but count against 5.1's
    /// call limit
    pub(crate) frames_native: u32,
    /// frames the host holds below the main thread's (`host_entry_layout`)
    pub(crate) host_frames: u32,
    /// metamethod, `__pairs` and `__close` continuation frames on the
    /// running thread: no frame of PUC's (its metamethod call makes one
    /// frame, the callee's), so not counted in `frames_in_use`.
    /// Per-thread, saved with the coroutine context.
    pub(crate) meta_conts: u32,
}
