// CARVE-OUT: pre-existing god file, shrinking on every touch
//! The interpreter. Dispatch is a plain match over opcodes. Lua→Lua calls
//! share one loop and never recurse the Rust stack; only native↔Lua boundaries do (e.g. pcall).
//!
//! Varargs follow 5.5 semantics: a vararg call materializes a vararg table
//! (fields 1..n plus "n") kept in the function's own stack slot; `...`
//! expands from it and `...name` binds it. 5.1 LUAI_COMPAT_VARARG also
//! materializes a local `arg` table (see `proto.has_compat_vararg_arg`).

use crate::frontend::SyntaxError;
use crate::jit::send_compat::TArc;
use crate::numeric::{self, Num};
use crate::runtime::heap::GcHeader;
use crate::runtime::{
    AfterClose, CallFrame, CloseCont, ContKind, Coro, CoroStatus, Frame, Gc, Heap, LuaClosure,
    MetaAction, MetaCont, NativeCont, Table, TableError, UpvalState, Upvalue, Value,
};
use crate::version::LuaVersion;
use crate::vm::callstack::DbgKind;
use crate::vm::error::LuaError;
use crate::vm::isa::{Inst, Op};
use native_call::NativeKind;

mod arith;
#[cfg(test)]
mod cont_trap_tests;
mod fast;
mod index;
mod index_fast;
mod limits;
pub(crate) mod native_call;
mod num;
mod trace_close;
mod trace_dispatch;
mod trace_exit;
mod trace_exit_decode;
mod trace_record;
mod trace_start;
use num::*;
pub(crate) use num::{ArithOp, arith_num, str_to_num};

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
pub struct Vm {
    /// The GC heap owned by this VM. Embedders normally interact via the
    /// `Vm` methods (`load` / `call_value` / `set_global` / …) rather than
    /// the heap directly.
    pub heap: Heap,
    pub(crate) stack: Vec<Value>,
    pub(crate) frames: Vec<CallFrame>,
    /// Shadow of `self.frames.len()`. Synced on every push/pop in the
    /// `frames_push_sync`/`frames_pop_sync` helpers (debug-asserted on
    /// use). Not consumed by readers yet; it is scaffolding for replacing
    /// `frames: Vec<CallFrame>` with a flat `[CallFrame; MAX_FRAMES]`
    /// indexed by frames_top.
    frames_top: u32,
    /// open upvalues, sorted ascending by stack slot
    open_upvals: Vec<(u32, Gc<Upvalue>)>,
    /// to-be-closed slots, ascending
    tbc: Vec<u32>,
    /// logical stack top for multi-result sequences
    pub(crate) top: u32,
    globals: Gc<Table>,
    /// shared metatable for all strings (populated by the string lib)
    /// per-basic-type metatables (PUC luaT): indexed by `type_mt_slot`
    /// (0 nil, 1 boolean, 2 number, 3 string, 4 function); tables carry their
    /// own. Settable via debug.setmetatable.
    type_mt: [Option<Gc<Table>>; 5],
    /// pre-interned metamethod event names, indexed by `Mm`
    mm_names: Vec<Gc<crate::runtime::LuaStr>>,
    /// native↔Lua nesting depth (PUC C-stack guard analogue)
    c_depth: u32,
    /// number of live pcall/xpcall continuation frames on the running thread
    /// (PUC counts these against nCcalls). Bounds protected-call recursion the
    /// way `c_depth` bounds call_value recursion. Per-thread: saved/restored
    /// with the coroutine context, since continuations survive a yield.
    pcall_depth: u32,
    /// number of non-yieldable C calls in flight on the running thread (PUC's
    /// `L->nny`). A library callback that runs via synchronous Rust recursion
    /// (sort comparator, gsub replacement) cannot be continued across a yield,
    /// so it bumps this for its duration; `coroutine.yield` inside hits the
    /// C-call boundary and errors. Always 0 at a suspend point (a yield can
    /// never cross such a call), so it needs no per-thread save/restore.
    nny: u32,
    /// Nonzero while an xpcall message handler is on the Rust stack. Used so a
    /// stack-overflow that surfaces *inside* the handler is reported as PUC's
    /// "error in error handling" (LUA_ERRERR + `luaD_seterrorobj`), not the
    /// plain "stack overflow" — errors.lua :606's `checkerr("error handling",
    /// loop)` then matches. PUC tracks this via the soft-cap window
    /// `nCcalls >= MAXCCALLS/10*11`; luna's c_depth is strict, so we mark the
    /// scope explicitly.
    pub(crate) msgh_depth: u32,
    /// set by a coroutine closing itself (`coroutine.close()` on the running
    /// thread): the to-be-closed handlers have already run; the thread must now
    /// terminate. `Some(None)` is a clean close, `Some(Some(e))` a handler
    /// raised `e`. Checked by `exec_with`/`resume_coro` to propagate (not
    /// unwind, so a protecting pcall cannot catch it) the termination.
    terminating: Option<Option<Value>>,
    /// xoshiro256** state (math.random)
    rng: [u64; 4],
    /// VM creation time (os.clock)
    started: std::time::Instant,
    version: LuaVersion,
    /// error object being threaded through a chain of __close handlers; a GC
    /// root for the duration (a handler may trigger collection)
    closing_err: Option<Value>,
    /// the coroutine whose context is currently live in the fields above;
    /// `None` while the main thread runs
    pub(crate) current: Option<Gc<crate::runtime::Coro>>,
    /// the main thread's saved execution context while a coroutine runs
    main_ctx: Option<SavedCtx>,
    /// set by `coroutine.yield` to suspend the running coroutine: the yielded
    /// values plus the slot/result-count needed to finish the yielding call on
    /// the next resume. Checked by `exec` to propagate (not unwind) on yield.
    yielding: Option<(Vec<Value>, u32, i32)>,
    /// results expected by the in-flight native call (so `yield` knows how many
    /// values its call site wants when it suspends)
    native_nresults: i32,
    /// identity object for the main thread, returned by `coroutine.running`
    /// (the main thread's context lives in the VM fields / `main_ctx`, not here)
    main_coro: Option<Gc<Coro>>,
    /// `collectgarbage` mode name ("incremental"/"generational"). The collector
    /// itself is still stop-the-world mark-sweep; this tracks the mode so mode
    /// switches report the previous one, as PUC does.
    gc_mode: &'static str,
    /// the live-register boundary of the running thread for GC rooting (PUC's
    /// `L->top`): set precisely at each GC safe point so freed temporary
    /// registers above it are not rooted. Without this the collector roots the
    /// whole stack window, pinning weak-table values stranded in stale temps
    /// (e.g. closure.lua's `while x[1]` GC-detection loop).
    pub(crate) gc_top: u32,
    /// `collectgarbage("param", name [,value])` pacing parameters. The collector
    /// is still stop-the-world, so these are stored/returned for API fidelity
    /// (PUC round-trips them via `setparam`/`getparam`). Defaults mirror PUC's
    /// `LUAI_GC*` knobs: pause=200, stepmul=100, stepsize=13.
    gc_pause: i64,
    gc_stepmul: i64,
    gc_stepsize: i64,
    /// `collectgarbage`'s parameters as the dialect stores them; they set
    /// the three knobs above through `set_gc_pacing`.
    pub(crate) gc_params: crate::vm::lib_gc::GcParams,
    /// true while `__gc` finalizers are being run, so a finalizer that calls
    /// `collectgarbage` gets a no-op (PUC's non-reentrancy: lua_gc returns -1 →
    /// `collectgarbage` yields fail).
    gc_finalizing: bool,
    /// C ABI scratch (`capi` module): the host-visible value stack that C
    /// callers operate on via `lua_pushinteger` / `lua_tostring` / etc.
    /// Kept here (instead of in a separate `LuaState` wrapper) so the
    /// trampoline that bridges to a `LuaCFunction` can safely cast the
    /// Vm pointer it already holds to the public `*mut LuaState` type
    /// without any aliasing of `&mut Vm` against `&mut LuaState.vm`.
    pub capi_stack: Vec<crate::runtime::Value>,
    /// Pinned CString backing the pointer last returned by `lua_tostring`;
    /// valid until the next `lua_tostring` on the same Vm.
    pub capi_cstr_pin: Option<std::ffi::CString>,
    /// PUC 5.4+ warning system. Lua manual §6.1 `warn`: emitted messages
    /// concatenate across continuation calls until a non-`tocont` call
    /// flushes; the default warnf recognises `@on`/`@off` control messages
    /// and starts disabled. luna's `emit_warn` mirrors the default warnf
    /// behaviour and 5.4+ `__gc` errors are routed through it (5.1–5.3
    /// keep the older raise semantics).
    pub(crate) warn_state: WarnState,
    pub(crate) warn_buf: Vec<u8>,
    /// Embedding cooperative budget: a per-Vm tick counter that the run
    /// loop decrements once per dispatch turn. When it hits zero the loop
    /// raises a catchable "instruction budget exceeded" error so the embedder
    /// can yield control back to its caller (short-script eval, game
    /// frame budgets). `None` = unbounded; reset on each call via
    /// `set_instr_budget`.
    pub(crate) instr_budget: Option<i64>,
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
    /// In-process log of fully-emitted warnings (each entry = one flushed
    /// message, sans the "Lua warning: " prefix and trailing newline). Lets
    /// tests assert what was warned without scraping stderr.
    pub(crate) warn_log: Vec<Vec<u8>>,
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
    /// lua.c's `-E`: libraries opened from now on ignore the environment
    pub(crate) ignore_env: bool,
    /// the running thread's debug hook state (`debug.sethook`); per-thread,
    /// swapped with the execution context on a coroutine resume/yield
    pub(crate) hook: HookState,
    /// true while the hook itself runs, so its own execution fires no events
    /// (PUC clears the mask for the duration)
    pub(crate) in_hook: bool,
    /// PUC `trap`: the dispatch loop head has work beyond fetching the next
    /// instruction — an instruction budget, a memory cap or an armed hook.
    /// The loop head clears it when it finds none of them; whatever may
    /// create one sets it (set spuriously, it costs one slow iteration).
    pub(crate) trap: bool,
    /// arms the next Lua frame's `tailcalls` count (PUC `ci->u.l.tailcalls`),
    /// consumed by `push_frame`. `OP_TailCall` sets it to the caller's
    /// own tailcalls + 1 before begin_call so deeply tail-recursive chains
    /// accumulate the count instead of capping at 1.
    pub(crate) pending_tailcalls: u32,
    /// arms the next Lua frame's `ccmt` (its `__call` chain length), consumed
    /// by `push_frame`. `OP_TailCall` sets it to the reused activation's
    /// count; `begin_call` otherwise sets the chain it just resolved.
    pending_ccmt: u8,
    /// Name of the C native that just propagated an error (captured before
    /// the native is popped from `running_natives`). Lets a dying coroutine
    /// preserve `[C]: in function '<name>'` at the top of its traceback
    /// snapshot — PUC walks `luaG_funcnamefrompc` over a still-live ci, but
    /// luna's native frames are off-stack so we stash the name explicitly.
    pub(crate) errored_natives: Vec<crate::vm::callstack::ErroredNative>,
    /// Frames below this index are out of reach of the error handler of
    /// an `xpcall` (PUC `L->errfunc`): a protected call made from Rust — a
    /// finalizer, the handler itself — starts a fresh `errfunc` scope.
    pub(crate) msgh_floor: usize,
    /// The message handler that is running, if any. PUC's `luaG_errormsg`
    /// calls the handler with `L->errfunc` still set, so an error inside
    /// the handler (and not caught within it) calls the handler again, at
    /// the point of that error. Per thread, like `L->errfunc`.
    pub(crate) msgh_running: Option<Value>,
    /// How many message-handler runs have started; lets a run tell whether
    /// the error it got back was already handled by a nested run.
    pub(crate) msgh_runs: u64,
    /// The value the last `xpcall` handler produced for the error in
    /// flight, so the unwind that carries it to the `xpcall` does not
    /// run the handler again.
    pub(crate) msgh_applied: Option<Value>,
    /// Whether an error nothing catches should keep its traceback: not
    /// inside a protected call made from Rust, which discards it.
    pub(crate) keep_error_traceback: bool,
    /// PUC `CallInfo.u2.transferinfo`: index of the first transferred value
    /// (relative to the activation's func slot) and the number transferred.
    /// Set just before firing a call/return hook, read by `getinfo("r")`.
    pub(crate) hook_ftransfer: u16,
    pub(crate) hook_ntransfer: u16,
    /// metamethod event tag (e.g. "close") to attach to the next Lua frame
    /// pushed by `push_frame`; `close_slots` sets this before calling a
    /// `__close` handler so `debug.traceback` names it "metamethod 'close'"
    /// (PUC `CallInfo.u.l.tm`). Single-shot: `push_frame` consumes it.
    pending_tm: Option<crate::runtime::function::FrameTm>,
    /// `true` when the next `push_frame` is the user hook function itself,
    /// so `debug.getinfo(1).namewhat` resolves to `"hook"` (PUC
    /// `CIST_HOOKED`). `run_hook` arms it before dispatching the hook.
    pending_is_hook: bool,
    /// traceback of an error nothing in its thread catches, one line per
    /// stack level, taken where it was raised (see `raise_to_handler`): what
    /// the host gets from `take_error_traceback`, and what `debug.traceback`
    /// shows of the coroutine it kills. Cleared on a catch and at host-level
    /// `call_value` entry (`public_call_depth == 0`).
    pub(crate) error_traceback: Option<Vec<Vec<u8>>>,
    /// nesting depth of public `call_value` entries (host vs. internal). The
    /// outermost entry (depth 0) resets per-error state (`error_traceback`);
    /// internal calls (e.g. xpcall msgh, sort callback) preserve it.
    public_call_depth: u32,
    /// stack of native (`Value::Native`) closures currently running on the
    /// Rust call stack. `begin_call` pushes the closure before invoking
    /// `nc.f` and pops on return. Used by `arg_error` to detect a *nested*
    /// native call (PUC `ar.name == NULL` at level 0 because the level-0
    /// caller is C, not Lua) and qualify the running function's name via
    /// `pushglobalfuncname` (e.g. `'sort'` → `'table.sort'`).
    /// Each entry also records where the native sits on the value and
    /// frame stacks, so the debug interface can place it among the Lua
    /// activations as PUC's CallInfo chain would (see `callstack`).
    pub(crate) running_natives: Vec<crate::vm::callstack::NativeAct>,
    /// Index into `running_natives` where the running thread's own natives
    /// begin; the ones below belong to the threads that resumed it.
    pub(crate) natives_base: usize,
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
    pub(crate) host_roots: Vec<crate::vm::host_roots::HostRootSlot>,
    /// Recycled-slot index pool. `pin_host` pops the
    /// back if non-empty, else extends `host_roots`. Generation
    /// overflow at `u32::MAX` retires the slot (NOT pushed here).
    pub(crate) host_roots_free: Vec<u32>,

    /// GC-rooted scratch stack for `table.sort` (and any other
    /// builtin that needs a Rust-side `Vec<Value>` to outlive a user
    /// callback). Each entry is one in-flight working buffer; `gc_roots`
    /// extends with every contained `Value` so a `collectgarbage()`
    /// inside the comparator cannot free strings/tables snapshotted
    /// here. Nested sorts push a new buffer on entry, pop on exit
    /// (sort.lua's `load(..)(); collectgarbage()` compare callback
    /// regression).
    pub(crate) sort_scratch: Vec<Vec<Value>>,

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

    /// Classification of the most recent error raised on this Vm.
    /// Embedders read via [`Vm::error_kind`]; the dispatcher sets it
    /// at well-known sites (syntax errors, instr-budget trips, native
    /// callback errors, type errors).
    pub(crate) last_error_kind: crate::vm::error::LuaErrorKind,

    /// `(source_name, line)` of the most recent error. Set by the
    /// dispatcher / lexer / parser; cleared when a new call_value
    /// enters cleanly.
    pub(crate) last_error_source: Option<(String, u32)>,

    /// When `true`, `instr_budget` exhaustion in
    /// the dispatcher hot loop yields cooperatively (sets
    /// [`Vm::host_yield_pending`] + returns a sentinel `Err` walked up
    /// to `EvalFuture::poll`) instead of returning a real
    /// "instruction budget exceeded" error. Set by [`Vm::eval_async`]
    /// for the duration of the future; restored to `false` on
    /// `Poll::Ready`. The sync `Vm::eval` / `Vm::call_value` paths
    /// leave it `false` so budget exhaustion stays a real error there.
    pub(crate) async_mode: bool,

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

    /// Set by the dispatcher when an async-mode
    /// budget exhaustion fires; checked by `exec_with` (so the
    /// sentinel propagates without `unwind` running, mirroring
    /// `yielding.is_some()`) and by `call_value_impl` (so the call
    /// frames survive for the next poll). Cleared by `drive_one`
    /// after translating it to `DispatchOutcome::BudgetExhausted`.
    pub(crate) host_yield_pending: bool,

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

    /// Companion to `pending_async_native_fut`:
    /// the `(func_slot, nargs, nresults, gc_top)` quad needed to
    /// commit the future's eventual `Ok(nret)` back into the calling
    /// frame's expected result slots. Recorded by the dispatcher;
    /// consumed by [`Vm::commit_async_native_result`] after the
    /// future resolves.
    pub(crate) pending_async_native_ctx: Option<AsyncNativeCallCtx>,

    /// Identifies this Vm to the JIT storages it compiles through
    /// ([`crate::jit::JitStorage::claim`]).
    jit_owner_id: u64,
    /// Storages [`Vm::install_jit_storage`] replaced: code compiled into
    /// them may still be referenced by this Vm's functions, so they live
    /// as long as the Vm.
    retired_jit_storage: Vec<Box<dyn crate::jit::JitStorage>>,
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

/// Per-thread debug hook state (PUC `lua_State` hook/hookmask/basehookcount/
/// hookcount). `func` is the Lua hook; the booleans are the PUC mask bits.
#[derive(Clone, Copy, Default)]
pub struct HookState {
    /// the hook function (`None` when no hook is installed)
    pub func: Option<Value>,
    /// Rust-side debug hook. Fires alongside the Lua hook
    /// (Rust first); both can be installed simultaneously, but most
    /// embedders pick one.
    pub rust_func: Option<RustDebugHook>,
    /// LUA_MASKCALL — fire on function entry
    pub call: bool,
    /// LUA_MASKRET — fire on function return
    pub ret: bool,
    /// LUA_MASKLINE — fire on source-line change
    pub line: bool,
    /// LUA_MASKCOUNT — fire every `count_base` instructions
    pub count: bool,
    /// instruction count between count events (PUC basehookcount)
    pub count_base: i64,
    /// instructions left until the next count event (PUC hookcount)
    pub count_left: i64,
}

/// Rust-side debug hook callback. Receives the `Vm` plus a
/// classified event. The callback runs synchronously in the
/// dispatcher; the hook flag (`in_hook`) is set for its duration so
/// hook recursion is suppressed.
pub type RustDebugHook = fn(&mut Vm, RustHookEvent);

/// Classified debug event delivered to a [`RustDebugHook`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RustHookEvent {
    /// Function entry (`hook_call` analogue).
    Call,
    /// Function return (`hook_return` analogue).
    Return,
    /// Tail call entry (PUC 5.2+ separates this from a plain Call).
    TailCall,
    /// Source-line change (the `u32` is the 1-based line number).
    Line(u32),
    /// Instruction count event (fires every `count_base` instructions).
    Count,
}

/// Mask flags for [`Vm::set_rust_debug_hook`]. OR these to subscribe
/// to multiple event categories with a single hook installation.
pub const HOOK_MASK_CALL: u32 = 1;
/// Subscribe to function-return events.
pub const HOOK_MASK_RETURN: u32 = 2;
/// Subscribe to line-change events.
pub const HOOK_MASK_LINE: u32 = 4;
/// Subscribe to instruction-count events.
pub const HOOK_MASK_COUNT: u32 = 8;

/// A thread's swapped-out execution context (PUC per-thread stack state).
struct SavedCtx {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    open_upvals: Vec<(u32, Gc<Upvalue>)>,
    tbc: Vec<u32>,
    top: u32,
    pcall_depth: u32,
    hook: HookState,
    /// PUC `L->l_gt` — the thread's own globals table. Carried alongside
    /// the rest of the suspended state so each thread can keep its own
    /// `setfenv(0, env)` rewire without the swap leaking into another
    /// thread (5.1 closure.lua :177).
    globals: Gc<Table>,
}

/// Outcome of unwinding the call stack on an error (see `Vm::unwind`).
enum Unwound {
    /// caught by a pcall/xpcall continuation; resume running its caller
    Caught,
    /// caught by a continuation that was the entry-level activation; these are
    /// the call's (wrapped) results
    CaughtReturn(Vec<Value>),
    /// no protecting continuation up to `entry_depth`; propagate the error
    Propagated(LuaError),
}

/// Outcome of an index/newindex/comparison fast path: either a directly
/// computed result, or a metamethod (with the receiver it resolved against) the
/// caller must invoke — synchronously (C context) or yieldably (VM opcode).
enum MmOut {
    /// index → the looked-up value; newindex → done (raw set performed);
    /// comparison → the boolean result already known
    Done(Value),
    /// a metamethod to call; `recv` is the chain element it was found on (the
    /// extra args — key / value — are supplied by the caller)
    Mm { func: Value, recv: Value },
    /// ≤5.3 `a <= b` synthesised via `not __lt(b, a)` when neither operand
    /// carries `__le` — `op_compare` swaps the args and negates the result.
    /// Lives separate from `Mm` so the synth path can stay yieldable without
    /// every other Mm caller learning a swap flag they would never set.
    CompareSynth { func: Value },
}

/// Metamethod events; discriminants index `Vm::mm_names`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum Mm {
    Index,
    NewIndex,
    Call,
    ToString,
    Metatable,
    Name,
    Eq,
    Lt,
    Le,
    Concat,
    Len,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    Unm,
    BNot,
    Close,
    Gc,
    Pairs,
}

// one absent bit per event in `Table::flags`
const _: () = assert!(MM_NAMES.len() <= 32);

const MM_NAMES: [&str; 28] = [
    "__index",
    "__newindex",
    "__call",
    "__tostring",
    "__metatable",
    "__name",
    "__eq",
    "__lt",
    "__le",
    "__concat",
    "__len",
    "__add",
    "__sub",
    "__mul",
    "__div",
    "__mod",
    "__pow",
    "__idiv",
    "__band",
    "__bor",
    "__bxor",
    "__shl",
    "__shr",
    "__unm",
    "__bnot",
    "__close",
    "__gc",
    "__pairs",
];

/// The metamethod event an opcode dispatches, without the `__` prefix (PUC
/// funcnamefromcode), for "(metamethod 'event')" call-error suffixes.
fn mm_event_name(op: crate::vm::isa::Op) -> Option<&'static str> {
    use crate::vm::isa::Op;
    Some(match op {
        Op::Add => "add",
        Op::Sub => "sub",
        Op::Mul => "mul",
        Op::Div => "div",
        Op::Mod => "mod",
        Op::Pow => "pow",
        Op::IDiv => "idiv",
        Op::BAnd => "band",
        Op::BOr => "bor",
        Op::BXor => "bxor",
        Op::Shl => "shl",
        Op::Shr => "shr",
        Op::Unm => "unm",
        Op::BNot => "bnot",
        Op::Concat => "concat",
        Op::Len => "len",
        Op::GetField | Op::GetTable | Op::GetI | Op::SelfOp => "index",
        Op::SetField | Op::SetTable | Op::SetI => "newindex",
        Op::Eq | Op::EqK => "eq",
        Op::Lt => "lt",
        Op::Le => "le",
        _ => return None,
    })
}

/// PUC MAXTAGLOOP (5.3+): bound on `__index`/`__newindex` chains.
const MAX_TAG_LOOP: u32 = 2000;
/// PUC `MAXCCMT`: bound on a `__call` metamethod chain (lvm.c). 200 chains
/// is more than any reasonable program needs and matches PUC 5.4/5.5; a
/// bound of `15` is tight enough to fire on calls.lua :194 (N=20).
const MAX_CCMT: u32 = 200;
/// PUC LUAI_MAXCCALLS analogue: native↔Lua nesting bound.
pub(crate) const MAX_C_DEPTH: u32 = 200;
/// Stack an xpcall handler may use past `MAX_LUA_STACK` while handling a
/// stack overflow: PUC's 200 extra `ERRORSTACKSIZE` slots, plus the frame
/// reserve (256) the overflowing call was refused, so the handler's first
/// frame fits where that one did not.
const ERROR_STACK_EXTRA: u32 = 200 + 256;
/// luna's engine-level VM stack cap (used by call-site overflow checks).
/// Slightly larger than PUC's `LUAI_MAXSTACK` so engine internals have a
/// little headroom above any single library push.
const MAX_LUA_STACK: u32 = 1 << 20;
/// PUC `LUAI_MAXSTACK` (`luaconf.h`): the cap library code consults via
/// `lua_checkstack` to refuse multi-value pushes (`table.unpack` returning
/// N values, `string.pack` results, etc.). 5.3 coroutine.lua :530 pins
/// this at one million — `for j in {lim-10, …}` expects every j ≥ lim-10
/// to fail because the few slots already consumed in the coroutine push
/// the effective cap below lim-10.
const PUC_MAXSTACK: i64 = 1_000_000;

/// PUC 5.4+ default warnf state. The base library's `warn` function flips
/// between `Off` and `On` via the `@on` / `@off` control messages; any other
/// `@<word>` control is silently ignored, mirroring `lauxlib.c::checkcontrol`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WarnState {
    /// `warn` calls are silently dropped (default after `warn("@off")`).
    Off,
    /// `warn` calls are delivered to stderr (after `warn("@on")`).
    On,
}

/// Best-effort extraction of a textual message from a `catch_unwind` payload.
/// `panic!("msg")` arrives as `String`, `panic!(static)` as `&str`; anything
/// else degrades to `"<non-string panic>"`. Used by the native-call
/// catch_unwind to fold the panic into a Lua error.
fn panic_payload_str(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    "<non-string panic>".to_string()
}

/// Combined error type returned by [`Vm::eval`] and friends — either the
/// chunk failed to parse / compile, or it raised at runtime.
#[derive(Debug)]
pub enum Error {
    /// Parse or compile failure.
    Syntax(SyntaxError),
    /// Runtime error raised during execution.
    Runtime(LuaError),
}

impl From<SyntaxError> for Error {
    fn from(e: SyntaxError) -> Error {
        Error::Syntax(e)
    }
}

impl From<LuaError> for Error {
    fn from(e: LuaError) -> Error {
        Error::Runtime(e)
    }
}

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

// Split-borrow free fn helpers for frames push/pop with shadow counter
// `frames_top: u32`. Free fns (not Vm methods) so callers can pass
// `&mut self.frames` + `&mut self.frames_top` as split borrows, allowing
// other `&mut self.field` reads inside the CallFrame construction (e.g.
// `std::mem::take(&mut self.pending_tm)`).
//
// The shadow has no readers yet; it just stays in sync + asserts.
//
// `trap` is the dispatch loop's slow-path flag: a continuation frame on top
// of the stack must be seen by the loop head, which tests nothing else
// unless `trap` is set. So pushing a continuation sets it (the protected
// call may finish without a frame of its own), and so does a pop that
// leaves one on top.
#[inline(always)]
fn frames_push_sync(
    frames: &mut Vec<CallFrame>,
    frames_top: &mut u32,
    trap: &mut bool,
    cf: CallFrame,
) {
    if matches!(cf, CallFrame::Cont(_)) {
        *trap = true;
    }
    frames.push(cf);
    // Shadow maintenance is debug-only: release builds skip the
    // increment + assertion entirely. While nothing reads the shadow,
    // its purpose is to VERIFY the assumed invariant
    // (frames_top == frames.len()) across all push/pop sites; once readers
    // consume it, release must run the increment unconditionally.
    #[cfg(debug_assertions)]
    {
        *frames_top += 1;
        debug_assert_eq!(
            *frames_top as usize,
            frames.len(),
            "P17-D frames_top out of sync after push",
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = frames_top;
}

#[inline(always)]
fn frames_pop_sync(
    frames: &mut Vec<CallFrame>,
    frames_top: &mut u32,
    trap: &mut bool,
) -> Option<CallFrame> {
    let r = frames.pop();
    if matches!(frames.last(), Some(CallFrame::Cont(_))) {
        *trap = true;
    }
    #[cfg(debug_assertions)]
    {
        if r.is_some() {
            *frames_top = frames_top.saturating_sub(1);
        }
        debug_assert_eq!(
            *frames_top as usize,
            frames.len(),
            "P17-D frames_top out of sync after pop",
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = frames_top;
    r
}

/// One-time env-var read for
/// `LUNA_AOT_PROBE`. Returns `true` iff the env var is set to any
/// non-empty value. The result is cached in a `OnceLock` so the
/// dispatcher's hot path pays a single atomic load per process. Off
/// by default — production deploys don't bleed diagnostic prints.
fn jit_probe_enabled() -> bool {
    static PROBE_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE_ON.get_or_init(|| {
        std::env::var("LUNA_AOT_PROBE")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
    })
}

impl Vm {
    /// Re-sync `frames_top` after a bulk `frames: Vec`
    /// swap (take_ctx, put_ctx, load_coro_ctx). Must be called after
    /// the Vec replacement to keep the shadow valid.
    #[inline(always)]
    fn frames_resync(&mut self) {
        // a thread switch swaps in that thread's hook
        self.trap = true;
        // Debug-only — see `frames_push_sync` comment.
        #[cfg(debug_assertions)]
        {
            self.frames_top = self.frames.len() as u32;
        }
    }

    // ====================================================================
    // Stack-inline frame metadata accessors (unused).
    //
    // These methods read/write the LJ_FR2 marker slots at `stack[base-2]`
    // (closure GCRef) and `stack[base-1]` (FrameMarker as i64). No call
    // site uses them yet.
    //
    // Preconditions (debug-asserted):
    // - base >= 2 (slots base-2 and base-1 must exist below the frame)
    // - self.stack.len() > base + max_stack (caller has grown stack)
    // - For Lua frames, stack[base-2] holds Value::Closure(cl)
    // - For Lua frames, stack[base-1] holds Value::Int(marker.to_raw())
    //
    // No release-build cost when unused (LTO strips dead methods).
    // ====================================================================

    /// Write a Lua frame's closure pointer into `stack[base-2]`.
    /// The caller must ensure `base >= 2` and the slot is within the
    /// stack's allocated range.
    #[inline]
    #[allow(dead_code)] // no consumer yet
    fn write_frame_closure(&mut self, base: u32, cl: crate::runtime::Gc<LuaClosure>) {
        debug_assert!(
            base >= 2,
            "frame closure slot needs base >= 2; got {}",
            base
        );
        let idx = (base - 2) as usize;
        debug_assert!(idx < self.stack.len(), "stack[base-2] out of range");
        self.stack[idx] = Value::Closure(cl);
    }

    /// Read a Lua frame's closure pointer from `stack[base-2]`.
    /// Returns `None` if the slot doesn't hold a closure (caller is
    /// expected to treat that as a corrupt frame).
    ///
    /// Uses the [`Value::tag_byte`] fast-path
    /// to avoid the enum-match cost on the hot path. Tag check via
    /// 1-byte load + branch + `as_closure_unchecked` payload load.
    #[inline]
    #[allow(dead_code)]
    fn read_frame_closure(&self, base: u32) -> Option<crate::runtime::Gc<LuaClosure>> {
        debug_assert!(base >= 2);
        let v = self.stack.get((base - 2) as usize)?;
        if v.tag_byte() == crate::runtime::value::tag::CLOSURE {
            // SAFETY: tag byte just verified == CLOSURE.
            Some(unsafe { v.as_closure_unchecked() })
        } else {
            None
        }
    }

    /// Write a packed [`FrameMarker`] into `stack[base-1]`. The marker
    /// encodes the frame kind (Lua / Cont) + PC-or-delta payload.
    /// Stored as `Value::Int(marker.to_raw())` so it round-trips
    /// cleanly through the value stack without losing bits.
    #[inline]
    #[allow(dead_code)]
    fn write_frame_marker(&mut self, base: u32, marker: crate::runtime::frame_marker::FrameMarker) {
        debug_assert!(base >= 1, "frame marker slot needs base >= 1; got {}", base);
        let idx = (base - 1) as usize;
        debug_assert!(idx < self.stack.len(), "stack[base-1] out of range");
        self.stack[idx] = Value::Int(marker.to_raw());
    }

    /// Read a packed [`FrameMarker`] from `stack[base-1]`. Returns
    /// `None` if the slot isn't a `Value::Int` (caller treats as a
    /// corrupt frame); the kind tag itself may still be invalid, in
    /// which case [`FrameMarker::kind`] returns `None` on the result.
    ///
    /// Uses the [`Value::tag_byte`] fast-path
    /// for the tag check + `as_int_unchecked` for the payload load.
    #[inline]
    #[allow(dead_code)]
    fn read_frame_marker(&self, base: u32) -> Option<crate::runtime::frame_marker::FrameMarker> {
        debug_assert!(base >= 1);
        let v = self.stack.get((base - 1) as usize)?;
        if v.tag_byte() == crate::runtime::value::tag::INT {
            // SAFETY: tag byte just verified == INT.
            Some(crate::runtime::frame_marker::FrameMarker::from_raw(
                unsafe { v.as_int_unchecked() },
            ))
        } else {
            None
        }
    }

    /// Build the raw `Vm` struct without main coroutine / RNG seed / library
    /// setup. Private helper shared by `Vm::new` and `Vm::new_minimal`; the
    /// caller is responsible for the rest of the bring-up.
    fn new_inner(version: LuaVersion) -> Vm {
        let mut heap = Heap::new();
        // PUC 5.1 had no ephemeron pass — `__mode='k'` tables marked their
        // values strongly. gc.lua's "weak tables" section relies on that.
        heap.no_ephemeron = version <= LuaVersion::Lua51;
        // PUC 5.3 needs two GC cycles to finalize a table caught in a
        // coroutine reference cycle (gc.lua :502); 5.4+ rewrote the GC and
        // finalize in a single cycle (5.4/5.5 gc.lua :544 assert exactly one).
        heap.defer_thread_cycle_finalize = version == LuaVersion::Lua53;
        let globals = heap.new_table();
        let mm_names = MM_NAMES.iter().map(|n| heap.intern(n.as_bytes())).collect();

        Vm {
            heap,
            stack: Vec::new(),
            frames: Vec::new(),
            frames_top: 0,
            open_upvals: Vec::new(),
            tbc: Vec::new(),
            top: 0,
            globals,
            type_mt: [None; 5],
            mm_names,
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
            capi_stack: Vec::new(),
            capi_cstr_pin: None,
            warn_state: WarnState::Off,
            warn_buf: Vec::new(),
            warn_log: Vec::new(),
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
            errored_natives: Vec::new(),
            msgh_floor: 0,
            msgh_running: None,
            msgh_runs: 0,
            msgh_applied: None,
            keep_error_traceback: true,
            hook_ftransfer: 0,
            hook_ntransfer: 0,
            pending_tm: None,
            pending_is_hook: false,
            error_traceback: None,
            public_call_depth: 0,
            running_natives: Vec::new(),
            natives_base: 0,
            // JIT-specific state lives in the `JitState`
            // sidecar. The `luna` crate's `Vm::new_minimal_with_jit` /
            // `install_jit_backend` / `luaL_newstate` swap in
            // `CraneliftBackend` for callers that want JIT acceleration.
            jit: crate::vm::jit_state::JitState::with_null_backend(),
            // host roots ticket pool for the `Lua` facade
            host_roots: Vec::new(),
            // MacroLua registry. Pre-populated with
            // built-ins (`@quote` / `@unquote` / `@if` / `@gensym`)
            // when this Vm is constructed under `LuaVersion::MacroLua`.
            macro_registry: if version == LuaVersion::MacroLua {
                crate::frontend::macro_expander::MacroRegistry::with_builtins()
            } else {
                crate::frontend::macro_expander::MacroRegistry::new()
            },
            host_roots_free: Vec::new(),
            sort_scratch: Vec::new(),
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
        let mut vm = Vm::new_inner(version);
        let mc = vm.heap.new_coro(Value::Nil, vm.globals);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
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
        crate::vm::builtins::open_base(self);
    }
    /// Install the `math` standard library.
    pub fn open_math(&mut self) {
        crate::vm::lib_math::open_math(self);
    }
    /// Install the `table` standard library.
    pub fn open_table(&mut self) {
        crate::vm::lib_table::open_table(self);
    }
    /// Install the `string` standard library (and the shared string metatable).
    pub fn open_string(&mut self) {
        crate::vm::lib_string::open_string(self);
    }
    /// Install the `utf8` standard library (5.3+).
    pub fn open_utf8(&mut self) {
        crate::vm::lib_utf8::open_utf8(self);
    }
    /// `os` and `io` are merged because file userdata shares state with both
    /// (`io.tmpname` and `os.tmpname` are the same function, `io.popen`
    /// wraps `os.execute`'s shell).
    pub fn open_os_io(&mut self) {
        crate::vm::lib_os_io::open_os_io(self);
    }
    /// Install the `debug` standard library (introspection / hooks). Off by
    /// default for sandbox embedders.
    pub fn open_debug(&mut self) {
        crate::vm::lib_debug::open_debug(self);
    }
    /// Install the `coroutine` standard library.
    pub fn open_coroutine(&mut self) {
        crate::vm::lib_coroutine::open_coroutine(self);
    }
    /// `package` plus the 5.1-only `module` and `package.seeall` aliases.
    pub fn open_package(&mut self) {
        crate::vm::lib_package::open_package(self);
    }
    /// 5.2-only `bit32` library (5.3+ retired in favour of native bitwise
    /// ops on 64-bit integers).
    pub fn open_bit32(&mut self) {
        crate::vm::lib_bit32::open_bit32(self);
    }

    /// xoshiro256** next.
    pub(crate) fn rng_next(&mut self) -> u64 {
        let s = &mut self.rng;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Seed the RNG via splitmix64 expansion (PUC randseed shape).
    pub(crate) fn rng_seed(&mut self, a: u64, b: u64) {
        // PUC setseed: state = [n1, 0xff, n2, 0] (0xff avoids an all-zero
        // state), then 16 discards to spread the seed. Matches PUC's exact
        // sequence so the low-level conformance test passes.
        self.rng = [a, 0xff, b, 0];
        for _ in 0..16 {
            self.rng_next();
        }
    }

    /// Wall-clock since VM creation (os.clock approximation).
    pub(crate) fn uptime(&self) -> std::time::Duration {
        self.started.elapsed()
    }

    /// Entropy for math.randomseed() with no arguments.
    pub(crate) fn rng_auto_seed(&mut self) -> (i64, i64) {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let addr = &self.rng as *const _ as u64;
        (t as i64, addr as i64)
    }

    /// Allocate a native function object (no upvalues): builtin registration.
    pub fn native(&mut self, f: crate::runtime::value::NativeFn) -> Value {
        Value::Native(self.heap.new_native(f, Box::new([])))
    }

    /// Allocate a native function object with captured upvalues.
    pub fn native_with(
        &mut self,
        f: crate::runtime::value::NativeFn,
        upvals: Box<[Value]>,
    ) -> Value {
        Value::Native(self.heap.new_native(f, upvals))
    }

    /// Install the shared string metatable (string library).
    pub fn set_string_metatable(&mut self, mt: Option<Gc<Table>>) {
        self.type_mt[3] = mt;
    }

    /// The current globals table (`_G` / `_ENV` source for new chunks).
    pub fn globals(&self) -> Gc<Table> {
        self.globals
    }

    /// Remaining VM stack slots (PUC `L->stack_last - L->top` analogue).
    /// Library code that pushes a known number of fresh slots — e.g.
    /// `table.unpack` returning N values — consults this to refuse when
    /// the push would blow past `LUAI_MAXSTACK`. 5.3 coroutine.lua :530's
    /// `for j in {lim-10, lim-5, …}` series pins this contract: the
    /// coroutine's already-built table eats a few slots, so an unpack of
    /// ~lim values can't fit.
    pub(crate) fn stack_room(&self) -> i64 {
        PUC_MAXSTACK - (self.stack.len() as i64)
    }

    /// Repoint the thread's "global table" used by *future* `Vm::load` calls
    /// for the chunk's `_ENV` upvalue (PUC 5.1 `setfenv(0, env)` rewrites
    /// `L->l_gt`). Already-loaded chunks keep their own snapshot via the
    /// per-closure cell-0 clone in `Op::Closure`, so they are unaffected.
    pub(crate) fn set_globals(&mut self, env: Gc<Table>) {
        self.globals = env;
    }

    /// The Lua dialect this VM was constructed for (5.1 / 5.2 / 5.3 / 5.4 /
    /// 5.5). Determines numeric semantics, available standard libraries, and
    /// metamethod behavior.
    pub fn version(&self) -> LuaVersion {
        self.version
    }

    /// Set a global by name. `v` may be any `IntoValue`: a primitive
    /// (`i64`, `f64`, `bool`, `&str`, `String`, `Vec<u8>`), a `Value`
    /// directly, an `Option<T>`, or a `Gc<Table>` / `Gc<LuaClosure>` /
    /// `Gc<NativeClosure>` handle.
    ///
    /// Returns `Err(LuaError)` only if the globals table overflows
    /// (extremely unlikely in practice — `MAX_ASIZE = 1 << 27`).
    /// String interning + key construction cannot fail.
    ///
    /// ```
    /// # use luna_core::vm::Vm;
    /// # use luna_core::version::LuaVersion;
    /// let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    /// vm.set_global("answer", 42).unwrap();
    /// vm.set_global("ratio", 0.5_f64).unwrap();
    /// vm.set_global("hello", "world").unwrap();
    /// let r = vm.eval("return answer, ratio, hello").unwrap();
    /// assert_eq!(r.len(), 3);
    /// ```
    pub fn set_global<V: crate::vm::IntoValue>(
        &mut self,
        name: &str,
        v: V,
    ) -> Result<(), LuaError> {
        let v = v.into_value(self);
        let k = Value::Str(self.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { self.globals.as_mut() }.set(&mut self.heap, k, v)?;
        self.heap
            .barrier_back(self.globals.as_ptr() as *mut crate::runtime::heap::GcHeader);
        Ok(())
    }

    /// Backward write barrier shorthand for native lib code: demote `t` from
    /// BLACK back to gray so the next propagate step re-traces its fields.
    /// No-op outside Propagate (parent is never BLACK at mutation time).
    pub(crate) fn barrier_back_table(&mut self, t: Gc<Table>) {
        self.heap
            .barrier_back(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
    }

    /// Forward write barrier shorthand: a closed upvalue is a single-slot
    /// container — `barrier_forward` is cheaper than `barrier_back` here.
    /// No-op outside Propagate.
    pub(crate) fn barrier_forward_upvalue(&mut self, uv: Gc<Upvalue>, child: Value) {
        self.heap
            .barrier_forward(uv.as_ptr() as *mut crate::runtime::heap::GcHeader, child);
    }

    /// Register a MacroLua macro under `name`. Inert
    /// under non-MacroLua dialects (the macro is stored but the load
    /// path only consults the registry when
    /// `self.version == LuaVersion::MacroLua`).
    ///
    /// `name` is stored without the leading `@` — source code writes
    /// `@double(x)` to invoke a macro registered as `"double"`.
    pub fn define_macro(&mut self, name: &str, m: Box<dyn crate::frontend::macro_expander::Macro>) {
        self.macro_registry.register(name, m);
    }

    /// Drop all MacroLua macros (built-in + custom).
    /// Mostly useful for tests.
    pub fn clear_macros(&mut self) {
        self.macro_registry.clear();
    }

    /// PUC `luaL_loadfilex`: compile the file `name` (standard input when
    /// `None`, named `stdin`) into a function. A first line starting with
    /// `#` is skipped; `mode` (`"t"`, `"b"`, `"bt"`, `None` for both)
    /// limits the chunk to text and/or binary. The error is the message
    /// PUC's function leaves: `cannot open <name>: <reason>` when the file
    /// cannot be read, or the positioned syntax error.
    pub fn load_file(
        &mut self,
        name: Option<&[u8]>,
        mode: Option<&[u8]>,
    ) -> Result<Value, LuaError> {
        crate::vm::lib_os_io::load_path(self, name, mode).map_err(LuaError)
    }

    /// PUC `luaL_loadbufferx`: compile `src` under `chunkname`, the chunk
    /// kind limited by `mode` as in [`Vm::load_file`]. A syntax error comes
    /// back as its positioned message (`<chunk id>:<line>: <message>`), the
    /// string `load` returns.
    pub fn load_buffer(
        &mut self,
        src: &[u8],
        chunkname: &[u8],
        mode: Option<&[u8]>,
    ) -> Result<Value, LuaError> {
        crate::vm::lib_os_io::load_chunk(self, src, chunkname, mode).map_err(LuaError)
    }

    /// Parse + compile a chunk and close it over the globals table.
    pub fn load(&mut self, src: &[u8], chunkname: &[u8]) -> Result<Gc<LuaClosure>, SyntaxError> {
        // Reject oversize input *before* handing the parser/lexer a
        // potentially multi-GB slice. The PUC-shaped `not enough memory`
        // message keeps `heavy.lua::loadrep` compatibility: that test
        // accepts either `string length overflow` or `not enough memory`
        // as the failure mode for a feeder loop that outruns the host
        // allocator. See `set_loader_input_budget`.
        if src.len() > self.loader_input_budget {
            return Err(SyntaxError {
                line: 0,
                msg: b"not enough memory".to_vec(),
            });
        }
        // a precompiled (binary) chunk is undumped; source is parsed + compiled
        let is_bytecode = crate::vm::dump::is_binary_chunk(src);
        if is_bytecode && !self.bytecode_loading {
            return Err(SyntaxError {
                line: 0,
                msg: b"attempt to load a binary chunk (bytecode loading disabled)".to_vec(),
            });
        }
        let proto = if is_bytecode {
            let allow_puc = self.puc_bytecode_loading;
            crate::vm::dump::undump_named(src, &mut self.heap, self.version, allow_puc, chunkname)
                .map_err(SyntaxError::unpositioned)?
        } else if self.version.is_macro_lua() {
            // MacroLua dialect: drain the lexer into a
            // token vec, run the macro expander pre-pass against the
            // per-Vm registry, then hand the rewritten stream to
            // `parse_tokens`. The AST + compiler are dialect-agnostic
            // because by this point all `@`/quote tokens are gone.
            let mut lexer = crate::frontend::lexer::Lexer::new(src, self.version);
            let mut raw: Vec<crate::frontend::token::TokenInfo> = Vec::new();
            loop {
                let t = lexer.next_token()?;
                let eof = matches!(t.tok, crate::frontend::token::Token::Eof);
                raw.push(t);
                if eof {
                    break;
                }
            }
            // Drop the trailing Eof — expander operates on the body and
            // `parse_tokens` reinserts Eof when it runs out of tokens.
            raw.pop();
            let expanded = self.macro_registry.expand(raw)?;
            let depth = self.c_depth + self.pcall_depth;
            let parsed =
                crate::frontend::parser::parse_tokens_at_depth(expanded, src, self.version, depth)?;
            crate::compiler::compile_parsed(
                &parsed.chunk,
                &parsed.end_lines,
                self.version,
                chunkname,
                &mut self.heap,
            )?
        } else {
            // PUC's `nCcalls` counts protected calls as well
            let depth = self.c_depth + self.pcall_depth;
            let parsed = crate::frontend::parser::parse_at_depth(src, self.version, depth)?;
            crate::compiler::compile_parsed(
                &parsed.chunk,
                &parsed.end_lines,
                self.version,
                chunkname,
                &mut self.heap,
            )?
        };
        // PUC `lua_load` (lapi.c) only seeds the loaded closure's first
        // upvalue with the globals table when the closure has *exactly* one
        // upvalue — that's the main-chunk `_ENV` case. A dumped non-main
        // function with two-or-more upvalues keeps every cell at nil; the
        // host must use `debug.setupvalue` to wire them up. 5.2 calls.lua
        // :293's `assert(x() == nil)` pins this contract.
        let n = proto.upvals.len();
        let mut ups: Vec<Gc<Upvalue>> = Vec::with_capacity(n.max(1));
        if n == 0 {
            // synthetic main chunk has no declared upvalues, but the engine
            // still expects at least one cell so the host can probe via
            // `debug.upvalueid` etc. Match the historical luna shape.
            ups.push(
                self.heap
                    .new_upvalue(UpvalState::Closed(Value::Table(self.globals))),
            );
        } else if n == 1 {
            ups.push(
                self.heap
                    .new_upvalue(UpvalState::Closed(Value::Table(self.globals))),
            );
        } else {
            for _ in 0..n {
                ups.push(self.heap.new_upvalue(UpvalState::Closed(Value::Nil)));
            }
        }
        Ok(self.heap.new_closure(proto, ups.into_boxed_slice()))
    }

    /// Compile and run `src` as an anonymous chunk; return its results.
    /// Source name in the traceback is `"=eval"`. Syntax errors are
    /// surfaced as `LuaError` carrying the formatted PUC-style message
    /// (interned through the heap so the error value composes with
    /// `pcall` / `error_text` like any runtime error).
    pub fn eval(&mut self, src: &str) -> Result<Vec<Value>, LuaError> {
        self.eval_chunk(src, "=eval")
    }

    /// Render an error value for messages/tests. Non-string errors —
    /// `error({code=…})`, `error(42)`, etc. — collapse to a type tag
    /// (`"(error object is a table value)"`); embedders that need
    /// structured payloads should inspect `e.0` directly. Errors whose
    /// text starts with `"native panic:"` indicate a Rust panic
    /// crossed `catch_unwind` — the Vm may be inconsistent and should
    /// be dropped (do not reuse).
    pub fn error_text(&self, e: &LuaError) -> String {
        match e.0 {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            v => format!("(error object is a {} value)", v.type_name()),
        }
    }

    /// Render an error value the way PUC's standalone `msghandler`
    /// does (lua.c): strings pass through, numbers stringify, and any
    /// other object is given a chance at its `__tostring` metamethod
    /// (the result must be a string) before collapsing to the
    /// `"(error object is a … value)"` tag. Needs `&mut self` because
    /// `__tostring` runs arbitrary Lua — `error_text` remains the
    /// non-executing variant.
    pub fn error_display(&mut self, e: &LuaError) -> String {
        match e.0 {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            v @ (Value::Int(_) | Value::Float(_)) => {
                String::from_utf8_lossy(&self.tostring_basic(v)).into_owned()
            }
            v => {
                let mm = self.get_mm(v, Mm::ToString);
                if !mm.is_nil()
                    && let Ok(r) = self.call_value(mm, &[v])
                    && let Some(Value::Str(s)) = r.first()
                {
                    return String::from_utf8_lossy(s.as_bytes()).into_owned();
                }
                format!("(error object is a {} value)", v.type_name())
            }
        }
    }

    /// Call `f` with `args` in protected mode with the message handler
    /// `msgh`: PUC `lua_pcall(L, nargs, LUA_MULTRET, msgh)` made from a C
    /// function of the host's, as lua.c's `docall` does from `pmain`.
    ///
    /// `msgh` runs where the error was raised, before the stack unwinds, so
    /// it can take a traceback of the failing call ([`Vm::traceback`]); an
    /// error inside it calls it again with the new error, as in PUC. The
    /// returned error carries what the handler returned.
    ///
    /// The call counts as one C level on the stack, the host function
    /// making it: `debug.getinfo` finds it below `f`, and a traceback taken
    /// inside ends with `[C]: in ?` (`[C]: ?` in 5.1).
    pub fn call_value_with_handler(
        &mut self,
        f: Value,
        args: &[Value],
        msgh: Value,
    ) -> Result<Vec<Value>, LuaError> {
        let level = self.native(crate::vm::builtins::nat_host_xpcall);
        let mut call_args = Vec::with_capacity(args.len() + 2);
        call_args.push(f);
        call_args.push(msgh);
        call_args.extend_from_slice(args);
        let mut results = self.call_value(level, &call_args)?;
        // the protected call's `true, results...` or `false, handled error`
        if results.first().is_some_and(|ok| ok.truthy()) {
            results.remove(0);
            Ok(results)
        } else {
            Err(LuaError(results.get(1).copied().unwrap_or(Value::Nil)))
        }
    }

    /// PUC `luaL_getmetafield`: the field `event` of `v`'s metatable, read
    /// raw; nil when `v` has no metatable or the field is absent.
    pub fn metafield(&mut self, v: Value, event: &str) -> Value {
        match self.metatable_of(v) {
            Some(mt) => {
                let key = Value::Str(self.heap.intern(event.as_bytes()));
                mt.get(key)
            }
            None => Value::Nil,
        }
    }

    /// Call any callable value from the host (or from natives like pcall).
    pub fn call_value(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        // host-level entry (no enclosing exec): drop any error state from a
        // prior call that propagated uncaught (`error_traceback` would
        // otherwise leak into the next debug.traceback call).
        if self.public_call_depth == 0 {
            self.error_traceback = None;
        }
        self.public_call_depth += 1;
        // JIT fast path. A host call with no args targeting a Lua
        // chunk whose body fits the int-arith whitelist short-circuits
        // the whole interpreter dispatch and runs straight through the
        // mmap'd native code. The lookup is one Cell::get + one match —
        // the slow path (compile attempt on first reach) is paid once per
        // Proto.
        if args.is_empty()
            && let Value::Closure(cl) = f
            && let Some(vs) = self.try_jit_call(cl)
        {
            self.public_call_depth -= 1;
            return Ok(vs);
        }
        let r = self.call_value_impl(f, args, true);
        self.public_call_depth -= 1;
        r
    }

    /// Peek/populate the Proto's JIT cache slot, returning
    /// `Some(values)` when the cached native fn is callable for a
    /// zero-arg call. (Non-zero-arg dispatch is handled by
    /// `try_jit_call_op` from inside `begin_call`.)
    fn try_jit_call(&mut self, cl: Gc<LuaClosure>) -> Option<Vec<Value>> {
        use crate::runtime::function::JitProtoState;
        if !self.jit.enabled {
            return None;
        }
        let proto = cl.proto;
        if let JitProtoState::Untried = proto.jit.get() {
            self.populate_jit_cache(proto);
        }
        match proto.jit.get() {
            JitProtoState::Compiled {
                entry,
                num_args: 0,
                returns_one,
                arg_float_mask: _,
                arg_table_mask: _,
                ret_is_float,
                ret_is_table,
            } => {
                // SAFETY: the source `*const u8` is a JIT-compiled function entry pointer produced by Cranelift with the target `fn`-pointer signature (IntChunkFn / IntFnN); the JitVmGuard above keeps the JIT_VM TLS slot live across the call.
                let f: crate::jit::IntChunkFn = unsafe { std::mem::transmute(entry) };
                // Install the active Vm + closure
                // for any Rust helper the JIT'd code may call (e.g.
                // `luna_jit_new_table`, `luna_jit_upval_get`) via
                // cranelift `Linkage::Import`. RAII clear on return.
                // Chunks with no upvalue reads don't touch the closure
                // slot, paying nothing.
                // Route through chunk_compiler so
                // the NullJitBackend path stays inert. Raw-ptr arg
                // avoids the &mut self borrow conflict against the
                // shared self.jit.chunk_compiler read.
                let vm_ptr: *mut Vm = self;
                let _jit_vm_guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                let r = unsafe { f() };
                drop(_jit_vm_guard);
                // A JIT helper may have detected a metatable
                // on a table operand and parked a deopt request here.
                // Discard the sentinel value and return None so the caller
                // re-runs the call through the interpreter, which honours
                // __index/__newindex.
                if self.jit.pending_err.take().is_some() {
                    return None;
                }
                Some(if returns_one {
                    let v = if ret_is_float {
                        Value::Float(f64::from_bits(r as u64))
                    } else if ret_is_table {
                        Value::Table(crate::runtime::Gc::from_ptr(
                            r as *mut crate::runtime::Table,
                        ))
                    } else {
                        Value::Int(r)
                    };
                    vec![v]
                } else {
                    Vec::new()
                })
            }
            // Non-zero-arg Compiled state: call_value's empty-args
            // fast path can't drive it. Op::Call handles those.
            JitProtoState::Compiled { .. } | JitProtoState::Failed | JitProtoState::Untried => None,
        }
    }

    /// Populate the cache slot. Flips `Untried` to either
    /// `Compiled { … }` or `Failed`; idempotent on already-populated
    /// states (call sites guard with a get before invoking).
    ///
    /// Consults a thread-local cross-`Vm` cache keyed by a hash of
    /// `proto.code`. Compiled artefacts live in the thread-local
    /// `JITModule` so their mmap pages outlive the `Vm`; subsequent
    /// `Vm`s loading the same source skip the cranelift compile step
    /// entirely.
    fn populate_jit_cache(&mut self, proto: Gc<crate::runtime::function::Proto>) {
        use crate::runtime::function::JitProtoState;
        let version = self.version();
        let pre53 = version <= crate::version::LuaVersion::Lua53;
        // 5.1 and 5.2 have no Int subtype (all numbers
        // are Float). The JIT's `GetUpval` ValueRead path uses this
        // to default-pin upvalue reads to Float without a tag check.
        let float_only = version <= crate::version::LuaVersion::Lua52;
        // Split-borrow JitState so the
        // trait method can take `&mut dyn JitStorage` without
        // double-borrowing self.jit.
        let jit = &mut self.jit;
        jit.storage.claim(self.jit_owner_id);
        let storage: &mut dyn crate::jit::JitStorage = jit.storage.as_mut();
        match jit
            .chunk_compiler
            .try_compile(storage, proto, pre53, float_only)
        {
            crate::jit::CompileResult::Compiled {
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            } => {
                proto.jit.set(JitProtoState::Compiled {
                    entry,
                    num_args,
                    returns_one,
                    arg_float_mask,
                    arg_table_mask,
                    ret_is_float,
                    ret_is_table,
                });
            }
            crate::jit::CompileResult::Skipped => {
                proto.jit.set(JitProtoState::Failed);
            }
        }
    }

    /// `Op::Call` JIT fast path. Run inside `begin_call`
    /// before `push_frame`. Returns `true` when the call was handled
    /// in-place (no new Lua frame). Constraints: every arg slot must
    /// be `Value::Int`, the cached arity must match the call site's
    /// `nargs`, the host wanted-count `wanted` is honoured by
    /// `finish_results`. Also bails when a debug hook is armed —
    /// JIT'd code does not fire line / call / return hooks, so any
    /// active hook makes the interpreter the source of truth.
    fn try_jit_call_op(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        wanted: i32,
    ) -> bool {
        use crate::runtime::function::JitProtoState;
        if !self.jit.enabled {
            return false;
        }
        // Any active debug hook means the interpreter has to run the
        // call so the hook gets the expected events.
        if self.hook.func.is_some() || self.hook.rust_func.is_some() {
            return false;
        }
        let proto = cl.proto;
        if let JitProtoState::Untried = proto.jit.get() {
            self.populate_jit_cache(proto);
        }
        let JitProtoState::Compiled {
            entry,
            num_args,
            returns_one,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        } = proto.jit.get()
        else {
            return false;
        };
        if num_args as u32 != nargs {
            return false;
        }
        // Pack args into i64 bit-patterns per the per-slot expected
        // kind. A Float-typed slot accepts Value::Float verbatim (and on
        // 5.1/5.2 promotes Value::Int(x) via i64 → f64); a Table-typed slot
        // accepts only Value::Table and passes the raw Gc ptr; an
        // Int-typed slot accepts only Value::Int. Any other shape
        // bails to the interpreter so the call's actual dynamics
        // (metamethod dispatch / type-coerce) take over.
        let mut args: [i64; crate::jit::MAX_JIT_ARITY as usize] =
            [0; crate::jit::MAX_JIT_ARITY as usize];
        // From 5.3 an integer is its own subtype: turned into a float for
        // a float-typed parameter, it would come back out (returned,
        // stored, printed) as a float. Only 5.1/5.2, where every number
        // is a float, may convert it.
        let int_as_float = self.version() <= crate::version::LuaVersion::Lua52;
        for i in 0..num_args as usize {
            let v = self.stack[(func_slot + 1) as usize + i];
            let want_float = (arg_float_mask >> i) & 1 == 1;
            let want_table = (arg_table_mask >> i) & 1 == 1;
            args[i] = match (want_table, want_float, v) {
                (true, _, Value::Table(t)) => t.as_ptr() as i64,
                (false, false, Value::Int(x)) => x,
                (false, true, Value::Float(f)) => f.to_bits() as i64,
                (false, true, Value::Int(x)) if int_as_float => (x as f64).to_bits() as i64,
                _ => return false,
            };
        }
        // Vm + closure pin for helpers, routed through chunk_compiler;
        // see the matching guard in `try_jit_call`.
        let vm_ptr: *mut Vm = self;
        let _jit_vm_guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
        // SAFETY: the source `*const u8` is a JIT-compiled function entry pointer produced by Cranelift with the target `fn`-pointer signature (IntChunkFn / IntFnN); the JitVmGuard above keeps the JIT_VM TLS slot live across the call.
        let r = unsafe {
            match num_args {
                0 => (std::mem::transmute::<*const u8, crate::jit::IntChunkFn>(entry))(),
                1 => (std::mem::transmute::<*const u8, crate::jit::IntFn1>(entry))(args[0]),
                2 => {
                    (std::mem::transmute::<*const u8, crate::jit::IntFn2>(entry))(args[0], args[1])
                }
                3 => (std::mem::transmute::<*const u8, crate::jit::IntFn3>(entry))(
                    args[0], args[1], args[2],
                ),
                4 => (std::mem::transmute::<*const u8, crate::jit::IntFn4>(entry))(
                    args[0], args[1], args[2], args[3],
                ),
                _ => unreachable!("MAX_JIT_ARITY enforces num_args <= 4"),
            }
        };
        drop(_jit_vm_guard);
        // See matching path in `try_jit_call`. A helper
        // flagged a metatable on a table operand; bail to the interpreter
        // so `push_frame` runs the call from scratch.
        if self.jit.pending_err.take().is_some() {
            return false;
        }
        // Write result at func_slot, replacing the closure value, then
        // hand to finish_results to pad/truncate per the call site's
        // `wanted` count.
        if returns_one {
            let v = if ret_is_float {
                Value::Float(f64::from_bits(r as u64))
            } else if ret_is_table {
                Value::Table(crate::runtime::Gc::from_ptr(
                    r as *mut crate::runtime::Table,
                ))
            } else {
                Value::Int(r)
            };
            self.stack[func_slot as usize] = v;
            self.finish_results(func_slot, 1, wanted);
        } else {
            self.finish_results(func_slot, 0, wanted);
        }
        true
    }

    /// `call_value` with control over the `from_c` debug boundary. A `__close`
    /// handler runs *within* the closing Lua frame's activation (PUC luaF_close
    /// invokes it inside that ci), so it is called with `from_c = false`: its
    /// debug parent is the closing function, not a synthetic C level.
    fn call_value_impl(
        &mut self,
        f: Value,
        args: &[Value],
        from_c: bool,
    ) -> Result<Vec<Value>, LuaError> {
        if self.c_depth >= MAX_C_DEPTH {
            // PUC `luaE_checkcstack`: at the limit the call fails; an xpcall
            // handler running on the error gets a tenth more room before its
            // own failure is "error in error handling"
            if self.msgh_depth == 0 {
                return Err(self.runerror("C stack overflow"));
            }
            if self.c_depth >= MAX_C_DEPTH / 10 * 11 {
                return Err(self.plain_err("error in error handling"));
            }
        }
        self.c_depth += 1;
        let func_slot = self.stack.len() as u32;
        self.stack.push(f);
        self.stack.extend_from_slice(args);
        self.top = self.stack.len() as u32;
        let r = self.call_at(func_slot, args.len() as u32, from_c);
        self.c_depth -= 1;
        if r.is_err()
            && self.yielding.is_none()
            && self.terminating.is_none()
            && !self.host_yield_pending
            && self.pending_async_native_fut.is_none()
        {
            // A `coroutine.yield` in flight raises a sentinel error to unwind the
            // Rust stack, but the suspended coroutine's frames/registers (which
            // sit at/above `func_slot`) must survive for the next resume — so we
            // only truncate on a real error. A self-close termination is in the
            // same boat: the dying thread's state is discarded wholesale.
            // A `host_yield_pending` cooperative yield is in
            // the same boat as `yielding`: the next `EvalFuture::poll`
            // resumes the same call, so the in-flight frames must
            // survive.
            self.stack.truncate(func_slot as usize);
            self.top = func_slot;
        }
        r
    }

    /// Invoke `f` with the running thread marked non-yieldable for the duration
    /// (PUC `luaD_callnoyield`): a `coroutine.yield` inside `f` hits the C-call
    /// boundary and errors instead of suspending. Used by library callbacks
    /// (sort comparator, gsub replacement) that run via synchronous Rust
    /// recursion and so could not be re-entered after a yield.
    pub(crate) fn call_noyield(
        &mut self,
        f: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, LuaError> {
        self.nny += 1;
        let r = self.call_value(f, args);
        self.nny -= 1;
        r
    }

    // ---- coroutines ----

    pub(crate) fn new_coro(&mut self, body: Value) -> Gc<Coro> {
        // The new coroutine inherits the creating thread's current globals
        // (PUC `lua_newthread`: the new state copies `g->mainthread`'s
        // `l_gt`). `Vm.globals` always reflects the live thread, so reading
        // it here picks the creator regardless of which coro is running.
        self.heap.new_coro(body, self.globals)
    }

    /// Is `t` the thread whose context is currently live in the VM?
    pub(crate) fn is_current_thread(&self, t: Option<Gc<Coro>>) -> bool {
        match (self.current, t) {
            (None, None) => true,
            (Some(a), Some(b)) => a.ptr_eq(b),
            _ => false,
        }
    }

    /// Read an open-upvalue slot from its owning thread's stack (the live VM
    /// stack if that thread is current, else its saved context).
    #[doc(hidden)]
    pub fn read_slot(&self, slot: u32, thread: Option<Gc<Coro>>) -> Value {
        let s = slot as usize;
        if self.is_current_thread(thread) {
            self.stack[s]
        } else {
            match thread {
                Some(co) => co.stack[s],
                None => self.main_ctx.as_ref().expect("main context").stack[s],
            }
        }
    }

    fn write_slot(&mut self, slot: u32, thread: Option<Gc<Coro>>, v: Value) {
        let s = slot as usize;
        if self.is_current_thread(thread) {
            self.stack[s] = v;
        } else {
            match thread {
                Some(co) => {
                    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                    unsafe { co.as_mut() }.stack[s] = v;
                    // co.stack is traced by Coro::trace; demote co back to
                    // gray so propagate re-traces this slot if it was
                    // already black.
                    self.heap
                        .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
                }
                None => self.main_ctx.as_mut().expect("main context").stack[s] = v,
            }
        }
    }

    /// Whether `co` is the main thread's identity object.
    pub(crate) fn is_main_coro(&self, co: Gc<Coro>) -> bool {
        self.main_coro.is_some_and(|m| m.ptr_eq(co))
    }

    /// The status of `co` from the caller's view. The main thread's identity
    /// object has no stored status — it is "running" when nothing else runs,
    /// else "normal" (it resumed the active coroutine).
    pub(crate) fn effective_coro_status(&self, co: Gc<Coro>) -> CoroStatus {
        if self.is_main_coro(co) {
            if self.current.is_none() {
                CoroStatus::Running
            } else {
                CoroStatus::Normal
            }
        } else {
            co.status
        }
    }

    /// `coroutine.close` (PUC `lua_closethread`): run the suspended coroutine's
    /// pending to-be-closed `__close` handlers, then mark it dead and drop its
    /// context. Handlers see the coroutine's death error (if it died by error)
    /// or nil; an error they raise propagates out. `Ok(Some(e))` means it died
    /// with error `e` and no handler overrode it; `Err` means a handler raised.
    pub(crate) fn close_coro(&mut self, co: Gc<Coro>) -> Result<Option<Value>, LuaError> {
        // re-entrant close: a __close handler closed its own coroutine while the
        // outer close is mid-flight (its context is live). Report success and let
        // the outer close finish — re-entering the swap would corrupt the stack.
        if self.current.is_some_and(|c| c.ptr_eq(co)) {
            return Ok(None);
        }
        // A chain of coroutines whose `__close` handlers each close the previous
        // one recurses on the C stack (PUC `luaD_callnoyield` in `lua_closethread`).
        // The calling handler's `call_value` has already pushed `c_depth` to the
        // cap, so here it reads as full first — report PUC's "C stack overflow"
        // before the next handler call would surface the plainer "stack overflow".
        if self.c_depth >= MAX_C_DEPTH {
            return Err(self.rt_err("C stack overflow"));
        }
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let death_err = unsafe { co.as_mut() }.error_value.take();
        // swap the caller's live context out (into a GC-rooted home) and the
        // coroutine's in, mirroring resume_coro, so the __close handlers run on
        // the coroutine's stack while everything stays rooted.
        let resumer = self.current;
        let rctx = self.take_ctx();
        match resumer {
            Some(r) => {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                let m = unsafe { r.as_mut() };
                m.stack = rctx.stack;
                m.frames = rctx.frames;
                m.open_upvals = rctx.open_upvals;
                m.tbc = rctx.tbc;
                m.top = rctx.top;
                m.pcall_depth = rctx.pcall_depth;
            }
            None => self.main_ctx = Some(rctx),
        }
        self.load_coro_ctx(co);
        self.current = Some(co);
        // PUC `luaE_resetthread` closes with no message handler, whatever
        // xpcall the coroutine was suspended in
        let natives_base = std::mem::replace(&mut self.natives_base, self.running_natives.len());
        let msgh_floor = std::mem::replace(&mut self.msgh_floor, self.frames.len());
        let result = self.close_slots(0, death_err);
        self.natives_base = natives_base;
        self.msgh_floor = msgh_floor;
        // discard the (now-closed) coroutine context and restore the caller
        let _ = self.take_ctx();
        match resumer {
            Some(r) => {
                self.load_coro_ctx(r);
                self.current = Some(r);
            }
            None => {
                let m = self.main_ctx.take().expect("main context saved");
                self.put_ctx(m);
                self.current = None;
            }
        }
        {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            let m = unsafe { co.as_mut() };
            m.status = CoroStatus::Dead;
            m.stack = Vec::new();
            m.frames = Vec::new();
            m.open_upvals = Vec::new();
            m.tbc = Vec::new();
            m.top = 0;
            m.pcall_depth = 0;
            m.resume_at = None;
            m.error_value = None;
            m.error_traceback = None;
            m.error_levels = None;
        }
        result.map(|()| death_err)
    }

    /// `coroutine.running`: the running thread plus whether it is the main one.
    pub(crate) fn running_thread(&self) -> (Value, bool) {
        match self.current {
            Some(co) => (Value::Coro(co), false),
            None => (Value::Coro(self.main_coro.expect("main coro")), true),
        }
    }

    /// `coroutine.isyieldable([co])`: whether `co` (default: the running
    /// thread) can yield. The main thread never can; any other coroutine can
    /// unless it is dead.
    pub(crate) fn is_yieldable(&self, co: Option<Gc<Coro>>) -> bool {
        match co {
            Some(c) => !self.main_coro.is_some_and(|m| m.ptr_eq(c)) && c.status != CoroStatus::Dead,
            // the running thread can yield only outside any non-yieldable C call
            None => self.current.is_some() && self.nny == 0,
        }
    }

    /// Why `coroutine.yield` may not suspend the running thread right now, as a
    /// PUC error message — `None` if it may. Distinguishes "not in a coroutine"
    /// from "inside an unyieldable C call" (sort/gsub callback).
    pub(crate) fn yield_barrier(&self) -> Option<&'static str> {
        // 5.1's pcall/xpcall are plain C calls (no continuations), so a yield
        // below one crosses the boundary like any other; 5.1 also has a single
        // wording for every case, the main thread included.
        // 5.1 also calls every metamethod and generic-for iterator through
        // `luaD_call`, which counts as a C level, so a yield from inside one
        // is refused as well.
        if self.version <= LuaVersion::Lua51 {
            let inside_call = self.frames.iter().enumerate().any(|(i, f)| match f {
                CallFrame::Cont(nc) => matches!(nc.kind, ContKind::Meta(_)),
                CallFrame::Lua(fr) => {
                    fr.tm.is_some()
                        || (i > 0
                            && self.frames[i - 1].lua().is_some_and(|c| {
                                let pc = (c.pc as usize).wrapping_sub(1);
                                c.closure
                                    .proto
                                    .code
                                    .get(pc)
                                    .is_some_and(|ins| ins.op() == Op::TForCall)
                            }))
                }
            });
            if self.current.is_none() || self.nny > 0 || self.pcall_depth > 0 || inside_call {
                return Some("attempt to yield across metamethod/C-call boundary");
            }
            return None;
        }
        if self.current.is_none() {
            Some("attempt to yield from outside a coroutine")
        } else if self.nny > 0 {
            Some("attempt to yield across a C-call boundary")
        } else {
            None
        }
    }

    /// The coroutine whose context is currently live (`None` on the main thread).
    pub(crate) fn current_coro(&self) -> Option<Gc<Coro>> {
        self.current
    }

    /// `coroutine.close()` on the *running* thread (PUC 5.5 close-self): run all
    /// its pending `__close` handlers, then signal termination. The handlers run
    /// here, in place, with the thread still non-yieldable (a yield in one hits
    /// the C-call boundary). The returned sentinel unwinds the Rust stack the
    /// way a yield does — `exec_with` propagates it past any protecting pcall
    /// rather than letting `unwind` catch it — and `resume_coro` turns it into a
    /// clean death (or, if a handler raised, the coroutine's error).
    pub(crate) fn close_running(&mut self) -> LuaError {
        let death = match self.close_slots(0, None) {
            Ok(()) => None,
            Err(e) => Some(e.0),
        };
        self.terminating = Some(death);
        LuaError(Value::Nil)
    }

    /// `coroutine.status` as seen by the caller.
    pub(crate) fn coro_status_str(&self, co: Gc<Coro>) -> &'static str {
        match self.effective_coro_status(co) {
            CoroStatus::Suspended => "suspended",
            CoroStatus::Running => "running",
            CoroStatus::Normal => "normal",
            CoroStatus::Dead => "dead",
        }
    }

    fn take_ctx(&mut self) -> SavedCtx {
        let saved = SavedCtx {
            stack: std::mem::take(&mut self.stack),
            frames: std::mem::take(&mut self.frames),
            open_upvals: std::mem::take(&mut self.open_upvals),
            tbc: std::mem::take(&mut self.tbc),
            top: self.top,
            pcall_depth: self.pcall_depth,
            hook: self.hook,
            globals: self.globals,
        };
        self.frames_resync(); // frames now empty
        saved
    }

    fn put_ctx(&mut self, c: SavedCtx) {
        self.stack = c.stack;
        self.frames = c.frames;
        self.open_upvals = c.open_upvals;
        self.tbc = c.tbc;
        self.top = c.top;
        self.pcall_depth = c.pcall_depth;
        self.hook = c.hook;
        self.globals = c.globals;
        self.frames_resync(); // sync shadow to new Vec
    }

    /// Move a coroutine's saved context into the live VM fields.
    fn load_coro_ctx(&mut self, co: Gc<Coro>) {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let m = unsafe { co.as_mut() };
        self.stack = std::mem::take(&mut m.stack);
        self.frames = std::mem::take(&mut m.frames);
        self.open_upvals = std::mem::take(&mut m.open_upvals);
        self.tbc = std::mem::take(&mut m.tbc);
        self.top = m.top;
        self.frames_resync(); // sync shadow to coro's frames
        self.pcall_depth = m.pcall_depth;
        self.hook = m.hook;
        self.globals = m.globals;
    }

    /// Save the live VM context back into a coroutine object.
    fn store_coro_ctx(&mut self, co: Gc<Coro>) {
        let c = self.take_ctx();
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let m = unsafe { co.as_mut() };
        m.stack = c.stack;
        m.frames = c.frames;
        m.open_upvals = c.open_upvals;
        m.tbc = c.tbc;
        m.top = c.top;
        m.pcall_depth = c.pcall_depth;
        m.hook = c.hook;
        m.globals = c.globals;
        // bulk-overwrite of every collectable field traced by Coro::trace:
        // demote the coro back to gray so propagate re-traces its new state.
        self.heap
            .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
    }

    /// `coroutine.resume` core: drive `co` with `args` until it yields, returns
    /// or errors. Ok(values) carries yielded or returned values; Err carries an
    /// error raised inside the coroutine (the coroutine becomes dead).
    pub(crate) fn resume_coro(
        &mut self,
        co: Gc<Coro>,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, LuaError> {
        match co.status {
            CoroStatus::Suspended => {}
            CoroStatus::Dead => return Err(self.plain_err("cannot resume dead coroutine")),
            _ => return Err(self.plain_err("cannot resume non-suspended coroutine")),
        }
        if self.c_depth >= MAX_C_DEPTH {
            return Err(self.plain_err("C stack overflow"));
        }
        self.c_depth += 1;
        let resumer = self.current;
        // save the resumer's live context away
        let rctx = self.take_ctx();
        match resumer {
            Some(r) => {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                let m = unsafe { r.as_mut() };
                m.stack = rctx.stack;
                m.frames = rctx.frames;
                m.open_upvals = rctx.open_upvals;
                m.tbc = rctx.tbc;
                m.top = rctx.top;
                m.pcall_depth = rctx.pcall_depth;
                m.globals = rctx.globals;
                m.status = CoroStatus::Normal;
                m.natives = self.natives_base..self.running_natives.len();
                // bulk overwrite of every traced field on r — mirror
                // store_coro_ctx's barrier_back so propagate re-traces r.
                self.heap
                    .barrier_back(r.as_ptr() as *mut crate::runtime::heap::GcHeader);
            }
            None => self.main_ctx = Some(rctx),
        }
        // swap the coroutine in
        self.load_coro_ctx(co);
        {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            let m = unsafe { co.as_mut() };
            m.status = CoroStatus::Running;
            m.resumer = resumer;
        }
        // co.resumer is a traced Gc field; barrier_back covers the new
        // resumer reference and any future field writes during this call.
        self.heap
            .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
        self.current = Some(co);
        let resumer_natives_base = self.natives_base;
        self.natives_base = self.running_natives.len();
        // the coroutine's own frames start a fresh reach for xpcall handlers
        let resumer_msgh_floor = std::mem::replace(&mut self.msgh_floor, 0);
        let resumer_msgh_running = self.msgh_running.take();
        // a coroutine that dies keeps its traceback for `debug.traceback(co)`
        let resumer_keeps_traceback = std::mem::replace(&mut self.keep_error_traceback, true);

        // drive it
        let drive = if co.started {
            self.coro_continue(&args)
        } else {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { co.as_mut() }.started = true;
            self.coro_first(co.body, &args)
        };

        // classify: a self-close termination or a pending yield each win over
        // the (sentinel) error they raised to unwind the Rust stack.
        let (outcome, status) = if let Some(death) = self.terminating.take() {
            // the coroutine closed itself: it dies now, cleanly or with the
            // error a `__close` handler raised.
            match death {
                Some(e) => {
                    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                    unsafe { co.as_mut() }.error_value = Some(e);
                    self.heap
                        .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
                    (Err(LuaError(e)), CoroStatus::Dead)
                }
                None => (Ok(Vec::new()), CoroStatus::Dead),
            }
        } else {
            match self.yielding.take() {
                Some((vals, fslot, nres)) => {
                    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                    unsafe { co.as_mut() }.resume_at = Some((fslot, nres));
                    (Ok(vals), CoroStatus::Suspended)
                }
                None => {
                    // died: a return is clean, an error is remembered so a later
                    // `coroutine.close` can report it (PUC lua_closethread).
                    // Keep the error-point traceback (taken by `unwind` before
                    // popping the failing frames) so `debug.traceback(co)` on
                    // the dead coroutine still shows the error site, as PUC's
                    // untouched dead stack does (db.lua :848 family).
                    if drive.is_err() {
                        let levels = self.error_traceback.take().unwrap_or_default();
                        let tb =
                            crate::vm::callstack::traceback_from_lines(self.version, &levels, 0);
                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                        let m = unsafe { co.as_mut() };
                        m.error_traceback = Some(tb);
                        m.error_levels = Some(levels);
                    }
                    if let Err(e) = drive {
                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                        unsafe { co.as_mut() }.error_value = Some(e.0);
                        self.heap
                            .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
                    }
                    (drive, CoroStatus::Dead)
                }
            }
        };

        // save the coroutine's context back and restore the resumer
        self.natives_base = resumer_natives_base;
        self.msgh_floor = resumer_msgh_floor;
        self.msgh_running = resumer_msgh_running;
        self.keep_error_traceback = resumer_keeps_traceback;
        self.store_coro_ctx(co);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { co.as_mut() }.status = status;
        match resumer {
            Some(r) => {
                self.load_coro_ctx(r);
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { r.as_mut() }.status = CoroStatus::Running;
                self.current = Some(r);
            }
            None => {
                let m = self.main_ctx.take().expect("main context saved");
                self.put_ctx(m);
                self.current = None;
            }
        }
        self.c_depth -= 1;
        outcome
    }

    /// First resume: install the body function at slot 0 and run.
    fn coro_first(&mut self, body: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        self.stack.clear();
        self.stack.push(body);
        self.stack.extend_from_slice(args);
        self.top = self.stack.len() as u32;
        match self.begin_call(0, Some(args.len() as u32), -1, true) {
            Ok(true) => self.exec_with(1),
            Ok(false) => Ok(self.take_results(0)),
            Err(e) => Err(e),
        }
    }

    /// Resume after a yield: deliver `args` as the results of the call that
    /// yielded, then continue the suspended thread.
    fn coro_continue(&mut self, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        let (fslot, nres) = self.current.unwrap().resume_at.expect("resume point");
        let n = args.len() as u32;
        // Restore the full register window of the suspended top frame: a yield
        // that unwound through a native (call_value) may have left the stack
        // shorter than the frame needs. `base + max_stack` is what push_frame
        // allocates; `fslot + n` covers the delivered yield results.
        let frame_need = self
            .frames
            .last()
            .and_then(CallFrame::lua)
            .map(|f| (f.base + f.closure.proto.max_stack as u32) as usize)
            .unwrap_or(0);
        let need = frame_need.max((fslot + n) as usize);
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        for (i, &v) in args.iter().enumerate() {
            self.stack[fslot as usize + i] = v;
        }
        self.finish_results(fslot, n, nres);
        // the suspended `coroutine.yield` (a C call) now returns its resume
        // values: fire the matching "return" hook PUC defers until the resume.
        self.hook_return(true, 1, n)?;
        self.exec_with(1)
    }

    /// `coroutine.yield`: suspend the running coroutine, recording where to
    /// resume. Errors if called outside a coroutine. Returns a sentinel error
    /// that `exec`/`resume_coro` recognise as a yield (never surfaced to Lua).
    pub(crate) fn do_yield(&mut self, func_slot: u32, vals: Vec<Value>) -> LuaError {
        let nres = self.native_nresults;
        self.yielding = Some((vals, func_slot, nres));
        // value is irrelevant: resume_coro consults `self.yielding`, not this
        LuaError(Value::Nil)
    }

    /// Install or clear the debug hook on the running thread (`debug.sethook`
    /// without a thread argument). Arms the calling frame's `oldpc` to the
    /// sethook CALL's own pc (one less than the next-to-execute pc), mirroring
    /// PUC `rethook`'s `L->oldpc = pcRel(savedpc, p)` (= savedpc - code - 1) on
    /// native return: the very next traceexec compares against the sethook
    /// CALL's line. When the install statement and the following statement are
    /// on different source lines (db.lua :322), `changedline` fires for that
    /// first statement; when they share a line (db.lua :25 wrapper), they do
    /// not, so the wrapper line is not re-fired.
    pub(crate) fn install_hook(&mut self, hook: HookState) {
        self.hook = hook;
        self.trap = true;
        if self.hook.line
            && let Some(f) = self.frames.last_mut().and_then(CallFrame::lua_mut)
        {
            f.hook_oldpc = f.pc.saturating_sub(1);
        }
    }

    /// Install a hook on `target` (`None`/current thread → the live VM fields;
    /// another, suspended thread → its saved `Coro` state). PUC `debug.sethook`
    /// with an optional thread argument.
    ///
    /// `target == None` means "no explicit thread argument" — PUC binds that
    /// to `L` (the running thread). luna's live VM fields (`self.hook`,
    /// `self.frames`, `self.stack`) ARE the running thread's state, regardless
    /// of whether that's the main thread or a currently-resumed coroutine
    /// (save/restore happens at resume/yield boundaries via `load_coro_ctx`/
    /// `store_coro_ctx`). So a `None` target should always route to
    /// `install_hook` on the live fields. The pre-fix predicate gate
    /// `is_current_thread(target)` returned `false` when running inside a
    /// coroutine (`self.current = Some(co)`, `target = None` don't match)
    /// and silently dropped the hook on the floor — the install happened on
    /// no thread at all.
    pub(crate) fn set_hook(&mut self, target: Option<Gc<Coro>>, state: HookState) {
        if target.is_none() || self.is_current_thread(target) {
            self.install_hook(state);
        } else if let Some(co) = target {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            let m = unsafe { co.as_mut() };
            m.hook = state;
            if state.line
                && let Some(f) = m.frames.last_mut().and_then(CallFrame::lua_mut)
            {
                f.hook_oldpc = u32::MAX;
            }
            // co.hook.func is a traced Value (Coro::trace covers it); demote
            // co back to gray so propagate sees the new hook function.
            self.heap
                .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
        }
    }

    /// The hook state of `target` (`None`/current → the live VM state).
    pub(crate) fn get_hook(&self, target: Option<Gc<Coro>>) -> HookState {
        match target {
            t if self.is_current_thread(t) => self.hook,
            Some(co) => co.hook,
            None => self.hook,
        }
    }

    /// Invoke the debug hook for `event` (PUC `luaD_hook`). The hook runs with
    /// hooks disabled (PUC clears the mask) and its results/stack growth are
    /// discarded so the interrupted frame's register window is untouched.
    /// `line` is the source line for a "line" event, `None` (→ nil) otherwise.
    fn run_hook(
        &mut self,
        event: &[u8],
        line: Option<i64>,
        from_native: bool,
    ) -> Result<(), LuaError> {
        // line and count events transfer no values (PUC `luaD_hook(L,
        // event, line, 0, 0)`); call and return hooks set theirs first
        if matches!(event, b"line" | b"count") {
            self.hook_ftransfer = 0;
            self.hook_ntransfer = 0;
        }
        // Rust hook fires first (no Vm reentrancy via call_value;
        // synchronous fn pointer call). Both Rust and Lua hooks may be
        // installed; both observe each event.
        if let Some(rh) = self.hook.rust_func {
            let evt = match event {
                b"call" => Some(RustHookEvent::Call),
                b"return" => Some(RustHookEvent::Return),
                b"tail call" | b"tail return" => Some(RustHookEvent::TailCall),
                b"line" => Some(RustHookEvent::Line(line.unwrap_or(0).max(0) as u32)),
                b"count" => Some(RustHookEvent::Count),
                _ => None,
            };
            if let Some(evt) = evt {
                let was_in_hook = self.in_hook;
                self.in_hook = true;
                // PUC `luaD_hook` roots the whole running frame while a hook
                // runs: a register written after the last safe point may sit
                // above `gc_top`, and the hook may collect
                let gc_top = self.gc_top;
                self.gc_top = gc_top.max(self.stack.len() as u32);
                rh(self, evt);
                self.gc_top = gc_top;
                self.in_hook = was_in_hook;
                self.trap = true;
            }
        }
        let Some(hook) = self.hook.func else {
            return Ok(());
        };
        let saved_top = self.top;
        let saved_len = self.stack.len();
        let name = Value::Str(self.heap.intern(event));
        let lv = line.map_or(Value::Nil, Value::Int);
        self.in_hook = true;
        // PUC `db_sethook`'s C trampoline `hookf` sits between the engine and
        // the Lua hook — so `getinfo(2)` inside the hook resolves to whatever
        // ci sat below `hookf` (the function being hooked). When that hooked
        // function is native, no Lua frame for it exists in luna's `frames`;
        // model it as a synthetic C level by pushing the hook with
        // `from_c = true` (then `c_frame_name` reads the caller's call
        // instruction → e.g. `name = "sethook"`). When the hooked function is
        // Lua (its frame is still on the stack), push with `from_c = false`
        // so the level descent lands on it directly. The hook's own frame
        // carries `is_hook = true` so `getinfo(1).namewhat` reports "hook"
        // (PUC `CIST_HOOKED`).
        self.pending_is_hook = true;
        let r = self.call_value_impl(hook, &[name, lv], from_native);
        self.pending_is_hook = false;
        self.in_hook = false;
        self.trap = true;
        self.stack.truncate(saved_len);
        self.top = saved_top;
        r.map(|_| ())
    }

    /// Fire the "call" hook on entry to a function, if armed and not already in
    /// a hook (PUC clears the mask while a hook runs). PUC's transferinfo for
    /// a call hook is the param window: ftransfer = 1, ntransfer = nargs.
    /// `is_tail` selects the "tail call" event (PUC `LUA_HOOKTAILCALL`); a
    /// tail-call hook has no matching return hook (PUC luaD_pretailcall).
    fn hook_call_with(
        &mut self,
        from_native: bool,
        nargs: u32,
        is_tail: bool,
    ) -> Result<(), LuaError> {
        if self.hook.call
            && !self.in_hook
            && (self.hook.func.is_some() || self.hook.rust_func.is_some())
        {
            self.hook_ftransfer = 1;
            self.hook_ntransfer = nargs.min(u16::MAX as u32) as u16;
            // PUC 5.1 didn't distinguish tail-call events — every call,
            // including tail-calls, fired plain `"call"`. 5.2 introduced
            // the separate `"tail call"` event (mask `"c"` covers both).
            // 5.1 db.lua :366 pins this with `{"call","call","call","call",
            // "return","tail return","return","tail return"}`.
            let event: &[u8] = if is_tail && self.version >= LuaVersion::Lua52 {
                b"tail call"
            } else {
                b"call"
            };
            self.run_hook(event, None, from_native)?;
        }
        Ok(())
    }

    pub(crate) fn hook_call(&mut self, from_native: bool, nargs: u32) -> Result<(), LuaError> {
        self.hook_call_with(from_native, nargs, false)
    }

    /// Fire the "return" hook on exit from a function, if armed. ftransfer is
    /// the first result slot relative to the activation's func slot, ntransfer
    /// the number of results.
    pub(crate) fn hook_return(
        &mut self,
        from_native: bool,
        ftransfer: u32,
        nresults: u32,
    ) -> Result<(), LuaError> {
        if self.hook.ret
            && !self.in_hook
            && (self.hook.func.is_some() || self.hook.rust_func.is_some())
        {
            self.hook_ftransfer = ftransfer.min(u16::MAX as u32) as u16;
            self.hook_ntransfer = nresults.min(u16::MAX as u32) as u16;
            self.run_hook(b"return", None, from_native)?;
        }
        Ok(())
    }

    /// PUC "tail return" event — fires once per tail call that collapsed
    /// into the activation now returning, *after* its own "return" event.
    /// 5.1 hook mask `"r"` covers both `return` and `tail return`.
    fn hook_tail_return(&mut self) -> Result<(), LuaError> {
        if self.hook.ret
            && !self.in_hook
            && (self.hook.func.is_some() || self.hook.rust_func.is_some())
        {
            self.run_hook(b"tail return", None, false)?;
        }
        Ok(())
    }

    /// Call a metamethod with a single expected result.
    fn call_mm1(&mut self, f: Value, args: &[Value]) -> Result<Value, LuaError> {
        let mut r = self.call_value(f, args)?;
        Ok(if r.is_empty() {
            Value::Nil
        } else {
            r.swap_remove(0)
        })
    }

    /// Begin a *yieldable* metamethod call from a VM instruction: `func(args…)`
    /// driven through the interpreter loop with a `Meta` continuation, so a
    /// `coroutine.yield` inside the metamethod suspends and resumes cleanly.
    /// On the metamethod's return the loop head runs `finish_meta(action, …)`.
    /// Returns to the caller with the call set up — the opcode arm must do no
    /// further work on the running frame and let the loop iterate. `tm` is
    /// the metamethod event name (e.g. "index", "add"); a Lua handler frame
    /// born from this call inherits it via `pending_tm`, so
    /// `debug.getinfo(1).namewhat == "metamethod"` and `.name == tm`
    /// (db.lua :878).
    fn begin_meta_call(
        &mut self,
        func: Value,
        args: &[Value],
        action: MetaAction,
    ) -> Result<(), LuaError> {
        let saved_top = self.top;
        let cont_slot = self.stack.len() as u32;
        self.stack.push(func);
        self.stack.extend_from_slice(args);
        self.top = self.stack.len() as u32;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Meta(MetaCont { action, saved_top }),
                func_slot: cont_slot,
                nresults: 1,
            }),
        );
        let saved_tm = self
            .pending_tm
            .replace(crate::runtime::function::FrameTm::Meta);
        // begin_call drives a Lua metamethod through the loop (returns true) or
        // runs a native one inline (returns false, leaving results at cont_slot
        // for the loop head to pick up); either way the Meta cont resolves there.
        let r = self.begin_call(cont_slot, Some(args.len() as u32), 1, true);
        // Native callees never consumed pending_tm (push_frame is only hit on
        // a Lua callee); restore so it doesn't leak to a later push_frame.
        self.pending_tm = saved_tm;
        r?;
        Ok(())
    }

    /// Apply a comparison opcode's outcome: a known boolean drives the
    /// conditional skip directly; a metamethod is called yieldably, its
    /// truthiness driving the skip on return.
    fn op_compare(&mut self, step: MmOut, l: Value, r: Value, k: bool) -> Result<(), LuaError> {
        match step {
            MmOut::Done(v) => self.cond_skip(v.truthy(), k),
            MmOut::Mm { func, .. } => {
                self.begin_meta_call(func, &[l, r], MetaAction::Compare { k, negate: false })?;
            }
            MmOut::CompareSynth { func } => {
                // ≤5.3 `__le` falls back to `not __lt(r, l)`; the swap and
                // negation are driven through `MetaAction::Compare` so the
                // metamethod call can yield like any other compare.
                self.begin_meta_call(func, &[r, l], MetaAction::Compare { k, negate: true })?;
            }
        }
        Ok(())
    }

    /// Complete a VM instruction whose metamethod just returned `result` (PUC
    /// `luaV_finishOp`). The running frame is already back on top.
    fn finish_meta(&mut self, action: MetaAction, result: Value) -> Result<(), LuaError> {
        match action {
            MetaAction::Store { dst } => self.stack[dst as usize] = result,
            MetaAction::Discard => {}
            MetaAction::Compare { k, negate } => {
                let t = if negate {
                    !result.truthy()
                } else {
                    result.truthy()
                };
                self.cond_skip(t, k);
            }
            MetaAction::Concat { dst, base_a } => {
                self.stack[dst as usize] = result;
                self.top = dst + 1;
                self.concat_run(base_a)?;
            }
        }
        Ok(())
    }

    // ---- metatables ----

    pub(crate) fn metatable_of(&self, v: Value) -> Option<Gc<Table>> {
        match v {
            Value::Table(t) => t.metatable(),
            Value::Userdata(u) => u.metatable(),
            v => type_mt_slot(v).and_then(|i| self.type_mt[i]),
        }
    }

    /// Set the shared metatable for `v`'s basic type (debug.setmetatable on a
    /// non-table). No-op for tables (they carry their own).
    pub(crate) fn set_type_metatable(&mut self, v: Value, mt: Option<Gc<Table>>) {
        if let Some(i) = type_mt_slot(v) {
            self.type_mt[i] = mt;
        }
    }

    /// The metamethod of `v` for `mm`, or nil.
    pub(crate) fn get_mm(&self, v: Value, mm: Mm) -> Value {
        match self.metatable_of(v) {
            Some(mt) => self.fast_tm(mt, mm),
            None => Value::Nil,
        }
    }

    /// `mt[mm]` through the absent-metamethod bits in `mt.flags` (PUC
    /// `fasttm`): a miss sets the event's bit, and any key the table gains
    /// clears them all.
    #[inline]
    pub(crate) fn fast_tm(&self, mt: Gc<Table>, mm: Mm) -> Value {
        let bit = 1u32 << mm as u32;
        if mt.flags & bit != 0 {
            return Value::Nil;
        }
        let v = mt.get_str(self.mm_names[mm as usize]);
        if v.is_nil() {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { mt.as_mut() }.flags |= bit;
        }
        v
    }

    /// PUC 5.1 `get_compTM`: a comparison metamethod (`__eq` / `__lt` / `__le`)
    /// only fires when both operands carry a metatable that exposes the same
    /// implementation. Returns the metamethod to call, or `Nil` when no
    /// compatible match exists. Used to honour events.lua 5.1 :262's rule
    /// that `c == d` (where `d` has no metatable) falls back to raw equality.
    pub(crate) fn get_comp_mm(&self, l: Value, r: Value, mm: Mm) -> Value {
        let mt1 = self.metatable_of(l);
        let Some(mt1) = mt1 else { return Value::Nil };
        let tm1 = self.fast_tm(mt1, mm);
        if tm1.is_nil() {
            return Value::Nil;
        }
        let mt2 = self.metatable_of(r);
        let Some(mt2) = mt2 else { return Value::Nil };
        if mt1.as_ptr() == mt2.as_ptr() {
            return tm1;
        }
        let tm2 = self.fast_tm(mt2, mm);
        if tm2.is_nil() {
            return Value::Nil;
        }
        if tm1.raw_eq(tm2) {
            return tm1;
        }
        Value::Nil
    }

    /// PUC `luaT_objtypename`: the type name shown in error messages. A table
    /// or full userdata whose metatable carries a string `__name` reports that
    /// (e.g. "FILE*", "My Type") instead of the bare "table"/"userdata".
    pub(crate) fn obj_typename(&self, v: Value) -> String {
        // `__name` (luaT_objtypename) arrived in 5.3
        if self.version >= LuaVersion::Lua53
            && matches!(v, Value::Table(_) | Value::Userdata(_))
            && let Value::Str(s) = self.get_mm(v, Mm::Name)
        {
            return String::from_utf8_lossy(s.as_bytes()).into_owned();
        }
        v.type_name().to_string()
    }

    fn call_at(
        &mut self,
        func_slot: u32,
        nargs: u32,
        from_c: bool,
    ) -> Result<Vec<Value>, LuaError> {
        let depth = self.frames.len();
        match self.begin_call(func_slot, Some(nargs), -1, from_c) {
            // run until every frame the call pushed has returned: a pcall /
            // xpcall / __pairs continuation sits below the frame of the
            // function it called, and it is the continuation that produces
            // the call's results (`true, ...`, or `false, msg` on an error)
            Ok(true) => self.exec_with(depth + 1),
            // native completed inline; results at func_slot..top
            Ok(false) => Ok(self.take_results(func_slot)),
            // pcall / xpcall pushed their continuation and then failed to
            // call their function (`pcall("x")`): the continuation catches
            // that error, as it does in the dispatch loop
            Err(e)
                if self.frames.len() > depth
                    && self.yielding.is_none()
                    && self.terminating.is_none()
                    && !self.host_yield_pending
                    && self.pending_async_native_fut.is_none() =>
            {
                match self.unwind(e.0, depth + 1) {
                    Unwound::Caught => self.exec_with(depth + 1),
                    Unwound::CaughtReturn(vals) => Ok(vals),
                    Unwound::Propagated(err) => Err(err),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Switch the `collectgarbage` mode, returning the previous mode name.
    pub(crate) fn gc_switch_mode(&mut self, new: &'static str) -> &'static str {
        std::mem::replace(&mut self.gc_mode, new)
    }

    /// Whether the current `collectgarbage` mode is "generational" (where a
    /// "step" is a minor collection — a full atomic pass — rather than a paced
    /// incremental sweep).
    pub(crate) fn gc_mode_is_generational(&self) -> bool {
        self.gc_mode == "generational"
    }

    /// Current `stepsize` pacing parameter (PUC: 0 means an unbounded step that
    /// completes a whole cycle at once).
    pub(crate) fn gc_stepsize(&self) -> i64 {
        self.gc_stepsize
    }

    /// Set luna's collector knobs: heap growth before a new cycle (%), sweep
    /// work per safe point, and the default step size (0 = a step completes
    /// the cycle).
    pub(crate) fn set_gc_pacing(&mut self, pause: i64, stepmul: i64, stepsize: i64) {
        self.gc_pause = pause;
        self.gc_stepmul = stepmul;
        self.gc_stepsize = stepsize;
    }

    /// Interpreter safe-point auto-GC: FULL incremental Propagate + adaptive
    /// paced sweep via `Vm::gc_step`.
    ///
    /// Running Propagate from a safe-point relies on objects being **born
    /// black during Propagate**: a newly allocated object never becomes
    /// dead-white at the atomic flip.
    ///
    /// Adaptive budget scales with heap size: 100M-object heap (heavy.lua's
    /// `loadrep` stress) gets a 25M-object budget so a cycle completes in
    /// O(SWEEP_DIVISOR) safe-points regardless of size.
    #[inline(always)]
    pub(crate) fn maybe_collect_garbage(&mut self, live_top: u32) {
        if !self.heap.gc_due() || self.gc_finalizing {
            return;
        }
        // Bare `live_top`, no `max(self.top)` widening: every frame-pop
        // site (`finish_results`, the Op::TailCall collapse, pcall
        // unwind) clears the slots it vacates, mirroring PUC's L->top
        // discipline.
        self.gc_top = live_top;
        // PUC stepmul: % of allocation rate. Higher = more GC work per
        // safe-point (lower memory, more CPU). Default 100 = `live / 4` per
        // step (~4 safe-points per cycle). stepmul=200 → `live / 2`, etc.
        const SWEEP_BASE: usize = 400; // 400 / stepmul=100 = divisor 4
        const MIN_BUDGET: usize = 64_000;
        let stepmul = self.gc_stepmul.max(1) as usize;
        let divisor = (SWEEP_BASE / stepmul).max(1);
        let budget = (self.heap.live_objects() / divisor).max(MIN_BUDGET);
        if self.gc_step(budget) {
            self.heap.rearm_gc_pause(self.gc_pause);
        }
    }

    /// The running stack's contract with the collector (PUC
    /// `traversethread`): the slots from `gc_top` up are dead when a cycle's
    /// marking ends, so they are cleared right then, before the sweep frees
    /// anything they point to. The other threads' stacks are marked whole.
    /// So every slot of every stack holds nil or a live value.
    fn clear_dead_stack(&mut self) {
        let lo = (self.gc_top as usize).min(self.stack.len());
        self.stack[lo..].fill(Value::Nil);
    }

    /// Enumerate the GC roots: first-class `Value` roots plus bare-object
    /// roots (open upvalues, which are not first-class Values). Shared by the
    /// full collector and the incremental-sweep driver so both snapshot the
    /// exact same live set.
    fn gc_roots(&self) -> (Vec<Value>, Vec<*mut GcHeader>) {
        let mut roots: Vec<Value> = Vec::with_capacity(self.stack.len() + 32);
        roots.push(Value::Table(self.globals));
        for mt in self.type_mt.into_iter().flatten() {
            roots.push(Value::Table(mt));
        }
        for &n in &self.mm_names {
            roots.push(Value::Str(n));
        }
        // Root the running thread's live registers (PUC marks [stack, top)).
        // `gc_top` is the instruction-level cursor of the last GC
        // safe-point: allocation safe-points set it via
        // `maybe_collect_garbage(live_top)`, and `begin_call` raises it
        // to the callee's argument top when entering a native — PUC's
        // `L->top = func + 1 + nargs` C-call discipline. Without that
        // raise, an explicit `collectgarbage()` collected with a STALE
        // cursor from some earlier (lower) safe-point and freed its own
        // caller's register-held strings
        // (STATUS_ACCESS_VIOLATION on Windows / ASAN heap-use-after-free
        // on Linux). Values stranded above the cursor stay
        // excluded so weak-table entries are not spuriously pinned
        // (gc.lua:544 suspended-coroutine collection).
        let live = (self.gc_top as usize).min(self.stack.len());
        roots.extend_from_slice(&self.stack[..live]);
        for cf in &self.frames {
            match cf {
                CallFrame::Lua(f) => roots.push(Value::Closure(f.closure)),
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Xpcall { handler },
                    ..
                }) => roots.push(*handler),
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Close(cc),
                    ..
                }) => {
                    // Root the error threaded through this close chain so a
                    // `collectgarbage()` inside a sibling `__close` handler
                    // does not free it before the next handler is invoked
                    // (PUC L->ci->u.l.errfunc / the closing_err shadow).
                    if let Some(e) = cc.pending {
                        roots.push(e);
                    }
                    if let AfterClose::ResumeUnwind { err, .. } = cc.after {
                        roots.push(err);
                    }
                }
                CallFrame::Cont(_) => {}
            }
        }
        if let Some(e) = self.closing_err {
            roots.push(e);
        }
        // Host roots — Lua-facade handles keep their referenced
        // values alive across calls/yields. Trace the whole vector;
        // unused slots (post-`unpin_all`) carry Value::Nil which the
        // GC ignores.
        for slot in &self.host_roots {
            // free-list slots carry Value::Nil (GC no-op)
            roots.push(slot.value);
        }
        // `table.sort` and similar builtins stash their working
        // `Vec<Value>` here so a `collectgarbage()` invoked inside the
        // comparator callback doesn't free strings/tables snapshotted
        // off the live table (sort.lua's `load(..)(); collectgarbage()`
        // compare regression).
        for buf in &self.sort_scratch {
            roots.extend_from_slice(buf);
        }
        // The running-natives chain holds Gc<NativeClosure>s
        // mid-execution. Without rooting them here, a `collectgarbage()`
        // invoked inside the running native (sort.lua's `load(..)();
        // collectgarbage()` compare callback regression) sweeps the
        // closure that's actively executing, leaving `nc.upvals`
        // dangling and the Rust local `nc` pointing at recycled memory
        // — the SIGSEGV pops on the very next field access or pop.
        for a in &self.running_natives {
            roots.push(Value::Native(a.nc));
        }
        // the running thread's debug hook (suspended threads root theirs via
        // Coro::trace / the main_ctx sweep below)
        if let Some(h) = self.hook.func {
            roots.push(h);
        }
        // the running coroutine (its saved-context fields live in the VM, but
        // the object itself + its resumer chain must stay reachable)
        if let Some(co) = self.current {
            roots.push(Value::Coro(co));
        }
        if let Some(mc) = self.main_coro {
            roots.push(Value::Coro(mc));
        }
        // debug.getregistry() and io library state
        if let Some(r) = self.registry {
            roots.push(Value::Table(r));
        }
        if let Some(mt) = self.file_mt {
            roots.push(Value::Table(mt));
        }
        if let Some(f) = self.io_input {
            roots.push(Value::Userdata(f));
        }
        if let Some(f) = self.io_output {
            roots.push(Value::Userdata(f));
        }
        if let Some(f) = self.io_stdin {
            roots.push(Value::Userdata(f));
        }
        // the main thread's saved context while a coroutine runs
        if let Some(m) = &self.main_ctx {
            roots.extend_from_slice(&m.stack);
            if let Some(h) = m.hook.func {
                roots.push(h);
            }
            for cf in &m.frames {
                match cf {
                    CallFrame::Lua(f) => roots.push(Value::Closure(f.closure)),
                    CallFrame::Cont(NativeCont {
                        kind: ContKind::Xpcall { handler },
                        ..
                    }) => roots.push(*handler),
                    CallFrame::Cont(_) => {}
                }
            }
        }
        let mut extra: Vec<*mut GcHeader> = self
            .open_upvals
            .iter()
            .map(|&(_, uv)| uv.as_ptr() as *mut GcHeader)
            .collect();
        if let Some(m) = &self.main_ctx {
            extra.extend(
                m.open_upvals
                    .iter()
                    .map(|&(_, uv)| uv.as_ptr() as *mut GcHeader),
            );
        }
        (roots, extra)
    }

    /// Run a full collection with the VM's roots, then run any `__gc`
    /// finalizers the collection scheduled. A no-op (returns 0) when already
    /// inside a finalizer — the collector is not reentrant (PUC).
    pub fn collect_garbage(&mut self) -> usize {
        if self.gc_finalizing {
            return 0;
        }
        self.clear_dead_stack();
        let (roots, extra) = self.gc_roots();
        let freed = self.heap.collect_ex(&roots, &extra);
        #[cfg(feature = "gc-verify")]
        self.verify_frame_regs_live("collect_garbage");
        self.run_finalizers();
        freed
    }

    /// `gc-verify`: after a collect, every register slot the
    /// collector just rooted (`[0, max(gc_top, top))` — the same bound
    /// `gc_roots` uses) must hold a live value. A dead value inside the
    /// rooted range means the root snapshot and the sweep disagreed —
    /// a use-after-free waiting to happen. (Slots ABOVE the bound may hold
    /// stale dead values legitimately; the interpreter's contract is
    /// that it writes them before reading.)
    #[cfg(feature = "gc-verify")]
    pub(crate) fn verify_frame_regs_live(&self, ctx: &str) {
        let live = self.heap.debug_live_set();
        let header = |v: Value| -> Option<usize> {
            match v {
                Value::Str(s) => Some(s.as_ptr() as usize),
                Value::Table(t) => Some(t.as_ptr() as usize),
                Value::Closure(c) => Some(c.as_ptr() as usize),
                Value::Native(n) => Some(n.as_ptr() as usize),
                Value::Coro(c) => Some(c.as_ptr() as usize),
                Value::Userdata(u) => Some(u.as_ptr() as usize),
                _ => None,
            }
        };
        let bound = (self.gc_top as usize).min(self.stack.len());
        for i in 0..bound {
            if let Some(h) = header(self.stack[i])
                && !live.contains(&h)
            {
                panic!(
                    "[gc-verify] {ctx}: rooted stack slot {i} (gc_top {}, top {}) \
                         holds a dead value {h:#x} after collect",
                    self.gc_top, self.top,
                );
            }
        }
        // Diagnostic tier: a dead value ABOVE the cursor is only a bug if
        // that register is a named local still in scope (the interpreter
        // WILL read it). Cross-check against the proto's LocVar table.
        for (fi, cf) in self.frames.iter().enumerate() {
            if let CallFrame::Lua(f) = cf {
                let base = f.base as usize;
                let maxs = f.closure.proto.max_stack as usize;
                let hi = (base + maxs).min(self.stack.len());
                let pc = f.pc;
                for i in bound.max(base)..hi {
                    if let Some(h) = header(self.stack[i])
                        && !live.contains(&h)
                    {
                        let reg = (i - base) as u32;
                        if let Some(lv) = f
                            .closure
                            .proto
                            .locvars
                            .iter()
                            .find(|lv| lv.reg == reg && lv.start_pc <= pc && pc < lv.end_pc)
                        {
                            panic!(
                                "[gc-verify] {ctx}: frame {fi} IN-SCOPE LOCAL '{}' \
                                     (reg {reg}, abs {i}, pc {pc}, gc_top {}) holds a \
                                     dead value {h:#x} — live_top cursor excluded a \
                                     live named local",
                                lv.name, self.gc_top,
                            );
                        }
                    }
                }
            }
        }
    }

    /// PUC 5.1 `collectgarbage` re-raised the first error a `__gc` finalizer
    /// threw; gc.lua's "errors during collection" probe relies on it. This
    /// variant runs the same cycle but propagates the captured finalizer
    /// error to the explicit caller.
    pub(crate) fn collect_garbage_propagating(&mut self) -> Result<usize, LuaError> {
        if self.gc_finalizing {
            return Ok(0);
        }
        self.clear_dead_stack();
        let (roots, extra) = self.gc_roots();
        let freed = self.heap.collect_ex(&roots, &extra);
        #[cfg(feature = "gc-verify")]
        self.verify_frame_regs_live("collect_garbage_propagating");
        self.run_finalizers_or_err()?;
        Ok(freed)
    }

    /// Whether a `__gc` finalizer is currently running (so `collectgarbage`
    /// should report fail rather than collect).
    pub(crate) fn gc_is_finalizing(&self) -> bool {
        self.gc_finalizing
    }

    /// PUC 5.4+ default warnf: emit one piece of a warning message. `to_cont`
    /// = true indicates more pieces follow (concatenated until the first
    /// `to_cont = false` call flushes the whole line). Mirrors
    /// `lauxlib.c::warnfon` + `warnfcont` + `checkcontrol`:
    ///   * If the buffer is fresh, `to_cont` is false, and the message is
    ///     `@<word>`, treat as a control message — only `@on` / `@off` are
    ///     recognised; any other `@…` is silently ignored.
    ///   * Otherwise, while the state is `Off`, drop the piece; while `On`,
    ///     accumulate, and flush to stderr + `warn_log` on the
    ///     non-continuation call.
    pub(crate) fn emit_warn(&mut self, msg: &[u8], to_cont: bool) {
        if self.warn_buf.is_empty()
            && !to_cont
            && let Some(b'@') = msg.first().copied()
        {
            match &msg[1..] {
                b"on" => self.warn_state = WarnState::On,
                b"off" => self.warn_state = WarnState::Off,
                _ => {} // unknown control — silently ignored (PUC checkcontrol)
            }
            return;
        }
        if self.warn_state == WarnState::Off {
            // drop continuation pieces too — PUC `warnfoff` is the trampoline
            return;
        }
        self.warn_buf.extend_from_slice(msg);
        if !to_cont {
            let line = std::mem::take(&mut self.warn_buf);
            eprintln!("Lua warning: {}", String::from_utf8_lossy(&line));
            self.warn_log.push(line);
        }
    }

    /// Drain the in-process warning log (one entry per emitted message, sans
    /// `"Lua warning: "` prefix and newline). For test harnesses that want to
    /// assert on warn output without scraping stderr.
    pub fn warn_log_take(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.warn_log)
    }

    /// Arm the cooperative instruction budget. The run loop
    /// decrements this once per dispatch turn; on zero it raises a catchable
    /// `"instruction budget exceeded"` error and disarms itself so the host
    /// can resume with a fresh budget on the next call. `None` removes the
    /// cap. Pass `Some(n)` before `eval`/`call_value` for the embedder's
    /// short-script semantics.
    pub fn set_instr_budget(&mut self, budget: Option<i64>) {
        self.instr_budget = budget;
        self.trap = true;
    }

    /// Remaining instruction budget (None when unbounded).
    pub fn instr_budget_remaining(&self) -> Option<i64> {
        self.instr_budget
    }

    /// Toggle the method JIT. Off on a Vm without a JIT backend, on once
    /// one is installed ([`Self::install_jit_backend`]); a value set here
    /// is kept across a later install. Sandbox embedders
    /// **must** disable JIT when relying on `instr_budget` — see the
    /// `jit_enabled` field doc for the rationale.
    pub fn set_jit_enabled(&mut self, enabled: bool) {
        self.jit.enabled = enabled;
        self.jit.enabled_chosen = true;
    }

    /// Current JIT enable state.
    pub fn jit_enabled(&self) -> bool {
        self.jit.enabled
    }

    /// Toggle the trace JIT. Same default and install rule as
    /// [`Self::set_jit_enabled`]. When enabled, hot
    /// back-edges are counted on `Proto.trace_hot_count`; once the
    /// counter passes `TRACE_HOT_THRESHOLD`, the dispatch loop enters
    /// recording mode at the back-edge target.
    pub fn set_trace_jit_enabled(&mut self, enabled: bool) {
        self.jit.trace_enabled = enabled;
        self.jit.trace_enabled_chosen = true;
    }

    /// Opt-in flag for the self-link cycle catch. See field
    /// docs for the correctness blocker. Default `false`.
    pub fn set_self_link_enabled(&mut self, enabled: bool) {
        self.jit.self_link_enabled = enabled;
    }

    /// Current state of the self-link cycle catch.
    pub fn self_link_enabled(&self) -> bool {
        self.jit.self_link_enabled
    }

    #[doc(hidden)]
    #[deprecated(since = "3.2.0", note = "renamed to `set_self_link_enabled`")]
    pub fn set_p16_self_link_enabled(&mut self, enabled: bool) {
        self.set_self_link_enabled(enabled);
    }

    #[doc(hidden)]
    #[deprecated(since = "3.2.0", note = "renamed to `self_link_enabled`")]
    pub fn p16_self_link_enabled(&self) -> bool {
        self.self_link_enabled()
    }

    /// Current trace-JIT enable state.
    pub fn trace_jit_enabled(&self) -> bool {
        self.jit.trace_enabled
    }

    /// Number of traces that have closed cleanly (looped back to the
    /// head PC) since this Vm was constructed. Cumulative; used by
    /// tests + tuning.
    pub fn trace_closed_count(&self) -> u64 {
        self.jit.counters.closed
    }

    /// Number of traces that have aborted (exceeded MAX_TRACE_LEN or
    /// hit an un-recordable op).
    pub fn trace_aborted_count(&self) -> u64 {
        self.jit.counters.aborted
    }

    /// Number of compiled traces whose close shape
    /// is `TraceEnd::InlineAbort` (depth>0 boundary). Such traces
    /// pin `dispatchable=false` because the dispatcher can't
    /// resume at a depth>0 PC without the matching CallFrames.
    /// The frame-materialisation helper could synthesise those, but
    /// the InlineAbort emit path isn't wired up to it.
    pub fn trace_inline_abort_count(&self) -> u64 {
        self.jit.counters.inline_abort
    }

    /// See `JitCounters::dispatch_off_reasons`.
    pub fn trace_dispatch_off_reasons(&self) -> &[&'static str] {
        &self.jit.counters.dispatch_off_reasons
    }

    /// See `JitCounters::compile_failed_reasons`.
    pub fn trace_compile_failed_reasons(&self) -> &[&'static str] {
        &self.jit.counters.compile_failed_reasons
    }

    /// See `JitCounters::closed_lens`. Returns
    /// `(is_call_triggered, ops_len)` for every trace that closed.
    pub fn trace_closed_lens(&self) -> &[(bool, usize)] {
        &self.jit.counters.closed_lens
    }

    /// See [`crate::vm::jit_state::JitCounters::close_cause_counts`].
    /// Per-reason close-cause counts (recorder-side abort/discard +
    /// lowerer-side dispatch_off labels) keyed by `&'static str`.
    pub fn trace_close_cause_counts(&self) -> &std::collections::HashMap<&'static str, u64> {
        &self.jit.counters.close_cause_counts
    }

    /// Number of compiled traces whose
    /// `CompiledTrace.downrec_link` is `Some(_)` (lowerer's
    /// `downrec_idx_opt` arm emitted the stitch sentinel + caller-pc
    /// guard scaffold).
    pub fn trace_downrec_link_compiled_count(&self) -> u64 {
        self.jit.counters.downrec_link_compiled
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::downrec_dispatched`]. Number
    /// of times the dispatcher's `is_downrec_sentinel` arm fired and
    /// classified the return as a caller-pc-guard HIT.
    pub fn trace_downrec_dispatched_count(&self) -> u64 {
        self.jit.counters.downrec_dispatched
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::downrec_deopt`]. Number of
    /// times the dispatcher entered a `downrec_link`-bearing trace and
    /// the trace returned via the lowerer's deopt block (caller-pc
    /// guard MISS), or the dispatcher itself force-deopted via the
    /// stitch-cycle checkpoint.
    pub fn trace_downrec_deopt_count(&self) -> u64 {
        self.jit.counters.downrec_deopt
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::multi_way_guard_emitted`].
    /// Number of compiled traces whose lowerer emitted a multi-way
    /// caller-pc guard chain (>= 2 distinct `caller_pc` candidates)
    /// at the `TraceEnd::DownRec` close + lifted `dispatchable = true`.
    pub fn trace_multi_way_guard_emitted_count(&self) -> u64 {
        self.jit.counters.multi_way_guard_emitted
    }

    /// Number of closed traces the lowerer compiled and
    /// parked on `Proto.traces`. Re-records of the same head_pc are
    /// deduped (the second close finds the head_pc already cached
    /// and skips compile), so this never exceeds `trace_closed_count`.
    pub fn trace_compiled_count(&self) -> u64 {
        self.jit.counters.compiled
    }

    /// Number of times the recorder captured a
    /// [`crate::jit::trace_types::FieldIcSnapshot`] under
    /// `LUNA_JIT_FIELD_IC=1`. Stays 0 on the env-default path. Used
    /// by the opt-in fire test to verify the env gate
    /// wiring round-trips end-to-end (env -> recorder -> snapshot
    /// -> counter -> getter -> assertion).
    pub fn trace_field_ic_snapshot_count(&self) -> u64 {
        self.jit.counters.field_ic_snapshot_captured
    }

    /// Number of closed traces the lowerer rejected
    /// (any of the bail conditions in
    /// `crate::jit::trace::try_compile_trace`).
    pub fn trace_compile_failed_count(&self) -> u64 {
        self.jit.counters.compile_failed
    }

    /// Number of times the dispatcher jumped into a
    /// compiled trace. Bumps on every entry; `trace_deopt_count`
    /// counts the subset where the trace returned with a parked
    /// `jit_pending_err`.
    pub fn trace_dispatched_count(&self) -> u64 {
        self.jit.counters.dispatched
    }

    /// Number of trace entries that came back with
    /// `jit_pending_err` set (typically a metatable shadowed an
    /// index inside a helper, forcing the dispatcher to fall back
    /// to the interpreter without committing the trace's result).
    pub fn trace_deopt_count(&self) -> u64 {
        self.jit.counters.deopt
    }

    /// Number of times the dispatcher started a side
    /// trace recording (an `exit_hit_counts` slot crossed
    /// [`crate::jit::trace::HOTEXIT_THRESHOLD`] while `active_trace`
    /// was None and trace JIT was enabled). Each unit is exactly one
    /// `start_side_trace` call; the actual compile success counts
    /// under [`Self::trace_compiled_count`] like any other trace.
    /// Probe use: distinguishes the "side-trace pipeline fired"
    /// signal from the "primary back-edge / call-trigger fired"
    /// signal without reading per-counter histograms.
    pub fn trace_side_trace_started_count(&self) -> u64 {
        self.jit.counters.side_trace_started
    }

    /// Number of side-trace recordings that closed,
    /// compiled successfully, AND patched their parent's
    /// `exit_side_trace_ptrs[exit_idx]`.
    pub fn trace_side_trace_compiled_count(&self) -> u64 {
        self.jit.counters.side_trace_compiled
    }

    /// Number of side traces that compiled
    /// successfully but were SHEDDED by the close-handler shape-
    /// match gate (`exit_tags_match_entry_tags`). High ratios
    /// vs. `trace_side_trace_compiled_count` indicate the
    /// architecture is shedding lots of would-be side traces;
    /// useful as a tuning probe for future relaxation of the
    /// gate or for child-IR re-specialisation against parent's
    /// exit shape.
    pub fn trace_side_trace_shape_mismatch_count(&self) -> u64 {
        self.jit.counters.side_trace_shape_mismatch
    }

    /// Sum of NewTable sites the pre-emit escape sweep
    /// classified as `crate::jit::trace::EscapeState::Sinkable`
    /// across every successfully compiled trace on this Vm. The
    /// count is post-demotion: sites pre-emit drops back to Escaped
    /// for not meeting the sunk-emit criteria are NOT counted.
    /// `trace_sunk_alloc_count` matches one-for-one today (every
    /// surviving Sinkable site goes through sunk emit).
    pub fn trace_sinkable_seen_count(&self) -> u64 {
        self.jit.counters.sinkable_seen
    }

    /// See `JitCounters::accum_bufferable_seen`.
    pub fn trace_accum_bufferable_seen_count(&self) -> u64 {
        self.jit.counters.accum_bufferable_seen
    }

    /// Total dispatch hits across all known traces,
    /// broken into hot-exit telemetry (max single-exit count,
    /// total dispatches, exit count). Used by probes to identify
    /// hot side-exits as side-trace candidates.
    ///
    /// Walks `cl.proto` AND all nested protos in `cl.proto.protos`
    /// recursively, so inner functions' traces are reported.
    pub fn trace_exit_hit_summary(
        &self,
        cl: crate::runtime::heap::Gc<crate::runtime::function::LuaClosure>,
    ) -> Vec<(u32, Vec<u32>)> {
        fn walk(
            proto: crate::runtime::heap::Gc<crate::runtime::function::Proto>,
            out: &mut Vec<(u32, Vec<u32>)>,
        ) {
            for ct in proto.traces.borrow().iter() {
                let counts: Vec<u32> = ct.exit_hit_counts.iter().map(|c| c.get()).collect();
                out.push((ct.head_pc, counts));
            }
            for inner in proto.protos.iter() {
                walk(*inner, out);
            }
        }
        let mut out: Vec<(u32, Vec<u32>)> = Vec::new();
        walk(cl.proto, &mut out);
        out
    }

    /// Surface every side-exit slot whose hit count is
    /// `>= HOTEXIT_THRESHOLD` across every trace reachable from
    /// `cl.proto` (recursively walking `proto.protos`). Returned
    /// entries are side-trace candidates: each carries the parent
    /// trace's `(head_proto, head_pc)`, the exit's index in the
    /// parent's `exit_hit_counts`, and the side trace's natural
    /// entry shape (`cont_pc` + `exit_tags`).
    ///
    /// Layout of `exit_hit_counts` (mirrored by the iter):
    /// - `[0..per_exit_inline.len())` → `InlineSideExit` (cont_pc +
    ///   window-sized exit_tags).
    /// - `[per_exit_inline.len()..inline.len() + per_exit_tags.len())`
    ///   → `per_exit_tags[i]` (per-cont_pc caller-window tags).
    /// - Last slot → global clean-tail (cont_pc = `head_pc`,
    ///   exit_tags = `ct.exit_tags`).
    pub fn hot_exit_iter(
        &self,
        cl: crate::runtime::heap::Gc<crate::runtime::function::LuaClosure>,
    ) -> Vec<crate::jit::trace::HotExitInfo> {
        use crate::jit::trace::{HOTEXIT_THRESHOLD, HotExitInfo};
        fn walk(
            proto: crate::runtime::heap::Gc<crate::runtime::function::Proto>,
            out: &mut Vec<HotExitInfo>,
        ) {
            for ct in proto.traces.borrow().iter() {
                let inline_n = ct.per_exit_inline.len();
                let tags_n = ct.per_exit_tags.len();
                debug_assert_eq!(
                    ct.exit_hit_counts.len(),
                    inline_n + tags_n + 1,
                    "exit_hit_counts layout invariant violated"
                );
                for (idx, cell) in ct.exit_hit_counts.iter().enumerate() {
                    let hits = cell.get();
                    if hits < HOTEXIT_THRESHOLD {
                        continue;
                    }
                    let (cont_pc, exit_tags) = if idx < inline_n {
                        let ent = &ct.per_exit_inline[idx];
                        (ent.cont_pc, ent.exit_tags.clone())
                    } else if idx < inline_n + tags_n {
                        let (pc, tags) = &ct.per_exit_tags[idx - inline_n];
                        (*pc, tags.clone())
                    } else {
                        (ct.head_pc, ct.exit_tags.clone())
                    };
                    out.push(HotExitInfo {
                        head_proto: proto,
                        head_pc: ct.head_pc,
                        exit_idx: idx,
                        hits,
                        cont_pc,
                        exit_tags,
                    });
                }
            }
            for inner in proto.protos.iter() {
                walk(*inner, out);
            }
        }
        let mut out: Vec<HotExitInfo> = Vec::new();
        walk(cl.proto, &mut out);
        out
    }

    /// Sum of NewTable sites that actually took the
    /// sunk-emit path across every successfully compiled trace on
    /// this Vm. Each counted site skips its heap `Gc<Table>`
    /// allocation per dispatch; the array part lives as Cranelift
    /// `Variable`s for the duration of the trace.
    pub fn trace_sunk_alloc_count(&self) -> u64 {
        self.jit.counters.sunk_alloc
    }

    /// Sum of materialise-helper emit sites across every
    /// successfully compiled trace on this Vm. Each unit is a
    /// (site × cmp side-exit) pair whose IR reconstructs a heap
    /// `Gc<Table>` from the virt slots on deopt.
    pub fn trace_materialize_emit_count(&self) -> u64 {
        self.jit.counters.materialize_emit
    }

    /// Diagnostic: total `Op::Closure` ops the trace JIT
    /// lowered to the `luna_jit_op_closure` helper. Each emitted op
    /// replaces a `Heap::new_closure_inline` call on the dispatch
    /// path; the count is static (one per matching op per compiled
    /// trace), summed at compile success.
    pub fn trace_closure_emit_count(&self) -> u64 {
        self.jit.counters.closure_emit
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::per_exit_inline_compiled`].
    /// Number of compiled traces whose `per_exit_inline.len() > 0`
    /// (depth>0 inlined cmp side-exits emitted).
    pub fn trace_per_exit_inline_compiled_count(&self) -> u64 {
        self.jit.counters.per_exit_inline_compiled
    }

    /// See
    /// [`crate::vm::jit_state::JitCounters::per_exit_inline_dispatchable`].
    /// Number of compiled traces with `per_exit_inline.len() > 0` AND
    /// `dispatchable == true` — i.e. the count of compiled traces
    /// that would actually exercise the AOT chain-reloc +
    /// deploy-resolver path.
    pub fn trace_per_exit_inline_dispatchable_count(&self) -> u64 {
        self.jit.counters.per_exit_inline_dispatchable
    }

    /// Diagnostic: max `inline_depth` ever seen on any
    /// `RecordedOp` pushed by the recorder. Tells tests + tuning
    /// whether a self-recursive function actually walked the depth
    /// tracker past 0. Saturates at `MAX_INLINE_DEPTH`. Persists
    /// across traces and Vm activations; reset only on `Vm::new`.
    pub fn trace_max_depth_seen(&self) -> u8 {
        self.jit.max_depth_seen
    }

    /// Last live Lua frame (the trace head's frame at
    /// dispatch time). The frame-materialization helper reads `.base`
    /// to compute offsets for each inlined frame's window.
    #[doc(hidden)]
    pub fn jit_last_lua_frame(&self) -> Option<Frame> {
        match self.frames.last() {
            Some(CallFrame::Lua(f)) => Some(*f),
            _ => None,
        }
    }

    /// Read-only borrow of the current call
    /// stack, for the [`crate::vm::inspect`] pure-read accessors used
    /// by `luna-tools` (`luna-profile`'s sampler walks this from
    /// inside a `Count` hook). Sibling-module scope: not part of the
    /// public embedder surface, but `inspect::frames_for_profile` is.
    #[doc(hidden)]
    pub(super) fn inspect_frames(&self) -> &[CallFrame] {
        &self.frames
    }

    /// Ensure the value stack covers indices
    /// `[0..need)`. Extends with Nil if shorter. Called by the
    /// frame-materialization helper before pushing an inlined frame
    /// whose register window may exceed the current stack length.
    #[doc(hidden)]
    pub fn jit_ensure_stack(&mut self, need: usize) {
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
    }

    /// Trace JIT path for `Op::Close A`. Predicts whether
    /// `__close` handlers would run (any active tbc slot ≥ from
    /// holding a non-nil/false Value); if so, returns 1 without doing
    /// anything and the trace side-exits at the op, so the interpreter
    /// runs the handlers. Otherwise performs the safe part of close —
    /// `close_from(from)` to close open upvals + drop any drained tbc
    /// entries ≥ from — and returns 0.
    ///
    /// Returns are i64-shaped so the cranelift import sig stays
    /// trivial (i64 → i64 mapping).
    #[doc(hidden)]
    pub fn jit_op_close(&mut self, start_offset: u32) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return 1;
        };
        let from = f.base + start_offset;
        let has_handler = self.tbc.iter().any(|&s| {
            s >= from && {
                let v = self.stack[s as usize];
                !matches!(v, Value::Nil | Value::Bool(false))
            }
        });
        if has_handler {
            self.jit.counters.deopt += 1;
            return 1;
        }
        self.close_from(from);
        // Drain any tbc entries ≥ from (they're nil/false stubs the
        // interpreter's drive_close would have skipped silently).
        while let Some(&s) = self.tbc.last() {
            if s < from {
                break;
            }
            self.tbc.pop();
        }
        0
    }

    /// Spill the trace's current value for a register to
    /// the underlying `vm.stack[base + slot_offset]`. Required before
    /// an `Op::Closure` whose inner proto has an `in_stack: true`
    /// upval at `slot_offset` — the helper's `find_or_create_upval`
    /// captures a live pointer to `vm.stack[base + slot_offset]`,
    /// which must hold the right value at call time (trace IR's
    /// Variable hasn't yet been written back).
    ///
    /// Parameters arrive as i64 from the IR: `slot_offset` is the
    /// caller-frame register index (`u32` in practice, depth=0
    /// only — depth>0 Closure is not supported); `tag` is the
    /// `crate::runtime::value::raw` byte for the slot's RegKind;
    /// `raw_bits` is the trace Variable's `use_var` payload
    /// (i64-shaped — Float is its bit-pattern, Table/Closure is the
    /// raw `Gc::as_ptr` cast).
    #[doc(hidden)]
    pub fn jit_spill_stack(&mut self, slot_offset: u32, tag: u8, raw_bits: u64) {
        let Some(f) = self.jit_last_lua_frame() else {
            self.jit.pending_err =
                Some(self.rt_err("JIT spill: no Lua frame on jit_last_lua_frame()"));
            return;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if self.stack.len() <= idx {
            self.stack.resize(idx + 1, Value::Nil);
        }
        // SAFETY: caller (trace JIT IR emit) provides matching
        // `(tag, raw_bits)` — same shape produced by Value::unpack.
        let v = unsafe {
            crate::runtime::Value::pack(tag, crate::runtime::value::RawVal { zero: raw_bits })
        };
        self.stack[idx] = v;
    }

    /// Refresh only the raw payload of
    /// `vm.stack[base + slot_offset]`, preserving its existing
    /// `Value` tag. The caller (trace JIT Op::Concat body emit)
    /// uses this when the slot's `RegKind` is `Unset` (no compile-
    /// time tag info; commonly `Str` slots which the trace doesn't
    /// model). The interp's previous execution of the same op
    /// already populated the slot with the right tag — the trace
    /// only needs to swap in its current raw value.
    #[doc(hidden)]
    pub fn jit_stack_update_raw(&mut self, slot_offset: u32, raw_bits: u64) {
        let Some(f) = self.jit_last_lua_frame() else {
            return;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if idx >= self.stack.len() {
            return;
        }
        let (tag, _) = self.stack[idx].unpack();
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        self.stack[idx] = unsafe {
            crate::runtime::Value::pack(tag, crate::runtime::value::RawVal { zero: raw_bits })
        };
    }

    /// Trace JIT path for `Op::Concat A B`.
    ///
    /// Mirrors the interp arm (this file ~L5112): `self.top =
    /// base + a + n; concat_run(base + a)`. Result lands at
    /// `vm.stack[base + a]`. Returns `0` on success, `-1` when the
    /// interpreter must do it (any error from `concat_run` OR
    /// detection that the metamethod path was taken — `concat_run`
    /// returns `Ok(())` after `begin_meta_call` which has pushed a Lua
    /// frame the trace can't safely continue past); the trace then
    /// side-exits at the op and the interpreter redoes it, raising the
    /// error or calling `__concat` itself.
    ///
    /// The frame-push detection uses `pre/post frames.len()` and
    /// unwinds any pushed frames first, so the exit sees a clean stack.
    #[doc(hidden)]
    pub fn jit_op_concat(&mut self, slot_offset: u32, n: i32) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return -1;
        };
        let abs_a = f.base + slot_offset;
        self.top = abs_a + n as u32;
        let pre_frames = self.frames.len();
        let result = self.concat_run(abs_a);
        let post_frames = self.frames.len();
        // Frame-push = metamethod path taken (begin_meta_call pushed
        // a Lua frame). The trace can't continue past it; unwind +
        // deopt so interp redoes Op::Concat in the slow path.
        while self.frames.len() > pre_frames {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        }
        if result.is_err() || post_frames > pre_frames {
            self.jit.counters.deopt += 1;
            return -1;
        }
        0
    }

    /// Pop a reusable `Vec<u8>` from the JIT
    /// accumulator buffer pool, returning a raw pointer. The trace
    /// fn's IR holds this pointer in a stack slot through the loop
    /// and calls `jit_str_buf_extend` per iter. If the pool is
    /// empty, allocate fresh.
    ///
    /// Safety: the returned pointer is valid until
    /// `jit_str_buf_release` is called or the Vm is dropped. The
    /// caller MUST not retain it across `enter_jit` boundaries.
    #[doc(hidden)]
    pub fn jit_str_buf_acquire(&mut self) -> *mut Vec<u8> {
        let buf = self.jit.str_buf_pool.pop().unwrap_or_default();
        // Move into a Box so the pointer is stable until release.
        Box::into_raw(Box::new(buf))
    }

    /// Return a previously-acquired buffer to the
    /// pool, dropping any excess past `jit_str_buf_pool_cap`. The
    /// buffer is `clear`ed (capacity retained) so the next acquire
    /// gets a ready-to-extend Vec.
    ///
    /// Safety: `buf` must have been returned by a prior
    /// `jit_str_buf_acquire` on the same Vm.
    #[doc(hidden)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // JIT helper: `buf` round-trips through `Box::into_raw`; SAFETY documented below.
    pub fn jit_str_buf_release(&mut self, buf: *mut Vec<u8>) {
        if buf.is_null() {
            return;
        }
        // SAFETY: `ptr` round-trips through `Box::into_raw` set up earlier in this dispatch (or owned by a long-lived VM handle); ownership re-acquired here.
        let mut owned = unsafe { Box::from_raw(buf) };
        owned.clear();
        if self.jit.str_buf_pool.len() < self.jit.str_buf_pool_cap {
            self.jit.str_buf_pool.push(*owned);
        }
        // Else: drop the buffer.
    }

    /// Append a LuaStr's bytes to the accumulator
    /// buffer. The trace IR computes the `str_ptr` (= raw bits of
    /// the piece slot) and passes it through; we treat it as a
    /// `*mut LuaStr` and append its bytes.
    ///
    /// Returns 0 on success, -1 if the piece isn't a Str (would
    /// trip __concat metamethod path → deopt to interp).
    ///
    /// Safety: `buf` from prior `acquire`; `str_ptr` from the
    /// trace's piece slot raw bits.
    #[doc(hidden)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // JIT helper: `buf` from prior `acquire`; `str_ptr` from trace piece slot; SAFETY documented below.
    pub fn jit_str_buf_extend(&mut self, buf: *mut Vec<u8>, str_ptr: i64) -> i64 {
        if buf.is_null() || str_ptr == 0 {
            return -1;
        }
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let buf = unsafe { &mut *buf };
        let lua_str_ptr = str_ptr as *const crate::runtime::string::LuaStr;
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let bytes = unsafe { crate::runtime::string::bytes_of(lua_str_ptr) };
        buf.extend_from_slice(bytes);
        0
    }

    /// Drain the accumulator buffer into a fresh
    /// `LuaStr` via `heap.intern`, returning the raw ptr bits for
    /// the trace to write into the accumulator slot.
    ///
    /// Returns the LuaStr ptr as i64 on success, 0 on overflow
    /// (the hard cap; the trace deopts).
    ///
    /// Safety: `buf` from prior `acquire`. The buffer is left
    /// CLEAR (drained) ready for `release`.
    #[doc(hidden)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // JIT helper: `buf` from prior `acquire`; SAFETY documented below.
    pub fn jit_str_buf_intern(&mut self, buf: *mut Vec<u8>) -> i64 {
        if buf.is_null() {
            return 0;
        }
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let buf = unsafe { &mut *buf };
        let bytes = std::mem::take(buf);
        // hard cap at 256KB
        if bytes.len() > 256 * 1024 {
            return 0;
        }
        let gc = self.heap.intern(&bytes);
        gc.as_ptr() as i64
    }

    /// Trace JIT helper for `Op::TForCall A 0 C`.
    ///
    /// Base path: copy R[A..=A+2] → R[A+4..=A+6] + `begin_call`.
    /// ipairs `inext` fast path at the top — skip begin_call
    ///     when R[A]=Native(ipairs_iter), R[A+1]=Table no-mt,
    ///     R[A+2]=Int.
    /// Batched out-ptr writeback — fill ctrl/key/val raws into
    ///     caller-provided buffers + return R[A+4]'s tag byte. Lets
    ///     emit skip 3 separate `luna_jit_stack_load` calls and 1
    ///     `luna_jit_stack_tag` call by reading the buffer via
    ///     cranelift `stack_load` IR instead. Returns -1 on deopt,
    ///     else R[A+4]'s tag byte | R[A+5]'s tag byte << 8 (the value's
    ///     tag only when `nvars >= 2`, 0 otherwise).
    #[doc(hidden)]
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // JIT helper: `ctrl_out`/`key_out`/`val_out` are caller-stack buffers from Cranelift-emitted prologue; SAFETY documented below.
    pub fn jit_op_tforcall(
        &mut self,
        slot_offset: u32,
        nvars: i32,
        ctrl_out: *mut i64,
        key_out: *mut i64,
        val_out: *mut i64,
    ) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return -1;
        };
        let abs = f.base + slot_offset;
        let need = (abs + 7) as usize;
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        // ipairs fast path
        let took_fast_path = if let Value::Native(n) = self.stack[abs as usize]
            && std::ptr::fn_addr_eq(
                n.f,
                crate::vm::builtins::ipairs_iter as crate::runtime::value::NativeFn,
            )
            && let Value::Table(t) = self.stack[(abs + 1) as usize]
            && t.metatable().is_none()
            && let Value::Int(i) = self.stack[(abs + 2) as usize]
        {
            let next_i = i.wrapping_add(1);
            let v = t.get_int(next_i);
            if v.is_nil() {
                self.stack[(abs + 4) as usize] = Value::Nil;
            } else {
                self.stack[(abs + 4) as usize] = Value::Int(next_i);
                if (nvars as usize) >= 2 {
                    self.stack[(abs + 5) as usize] = v;
                }
                for j in 2..nvars as usize {
                    let slot = abs + 4 + j as u32;
                    if (slot as usize) < self.stack.len() {
                        self.stack[slot as usize] = Value::Nil;
                    }
                }
            }
            true
        } else {
            false
        };
        if !took_fast_path {
            // slow path: copy R[A..=A+2] → R[A+4..=A+6], then
            // route through begin_call. Lua-closure iters would push
            // a Lua frame mid-trace → deopt.
            self.stack[(abs + 4) as usize] = self.stack[abs as usize];
            self.stack[(abs + 5) as usize] = self.stack[(abs + 1) as usize];
            self.stack[(abs + 6) as usize] = self.stack[(abs + 2) as usize];
            // the interpreter raises the call's error itself; and a native
            // that `begin_call` hands to the interpreter loop (pcall, xpcall,
            // pairs, an async native) pushes frames or parks a future
            // instead of returning its results here
            let runs_to_completion = match self.stack[abs as usize] {
                Value::Native(nc) => nc.kind == NativeKind::Plain,
                _ => false,
            };
            if !runs_to_completion || self.begin_call(abs + 4, Some(2), nvars, false).is_err() {
                self.jit.counters.deopt += 1;
                return -1;
            }
        }
        // Batched writeback — fill the caller's buffers with the
        // raw bits of R[A+2] / R[A+4] / R[A+5] so the trace IR can
        // reload via cranelift `stack_load` instead of separate
        // `luna_jit_stack_load` helper calls.
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let ctrl_raw = unsafe { self.stack[(abs + 2) as usize].unpack().1.zero };
        let (key_tag, key_rv) = self.stack[(abs + 4) as usize].unpack();
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let key_raw = unsafe { key_rv.zero };
        let (val_tag, val_raw) = if (nvars as usize) >= 2 {
            let (tag, rv) = self.stack[(abs + 5) as usize].unpack();
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            (tag, unsafe { rv.zero })
        } else {
            (0, 0u64)
        };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe {
            ctrl_out.write(ctrl_raw as i64);
            key_out.write(key_raw as i64);
            val_out.write(val_raw as i64);
        }
        i64::from(key_tag) | i64::from(val_tag) << 8
    }

    /// Load the raw `i64` payload of
    /// `vm.stack[base + slot_offset]` for the active trace's head
    /// Lua frame. Used to reload trace IR `Variable`s after a
    /// helper has written to `vm.stack` directly (e.g. TForCall's
    /// iter results land at `R[A+4..A+4+nvars]`).
    #[doc(hidden)]
    pub fn jit_stack_load(&mut self, slot_offset: u32) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return 0;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if idx >= self.stack.len() {
            return 0;
        }
        let v = self.stack[idx];
        let (_, raw) = v.unpack();
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { raw.zero as i64 }
    }

    /// Read the tag byte of
    /// `vm.stack[base + slot_offset]`. Used by `Op::TForLoop` emit
    /// to dispatch on the iterator's return-key tag at runtime
    /// (`raw::NIL` → loop end exit, `raw::INT` → continue, other →
    /// deopt).
    #[doc(hidden)]
    pub fn jit_stack_tag(&mut self, slot_offset: u32) -> u8 {
        let Some(f) = self.jit_last_lua_frame() else {
            return crate::runtime::value::raw::NIL;
        };
        let idx = (f.base as usize) + (slot_offset as usize);
        if idx >= self.stack.len() {
            return crate::runtime::value::raw::NIL;
        }
        self.stack[idx].unpack().0
    }

    /// Push a Lua frame onto the call stack with
    /// JIT-known metadata. Used by `luna_jit_trace_materialize_frames`
    /// at trace side-exits to recreate the inlined call activations
    /// the lowerer compiled past. The contract (enforced by the
    /// lowerer's pre-emit pass): `cl.proto` is non-vararg,
    /// `nresults` is the caller's expected count (today always 1
    /// because the lowerer bails Op::Call C != 2), and the caller
    /// has already called `jit_ensure_stack` to cover
    /// `[0..base + cl.proto.max_stack)`.
    #[doc(hidden)]
    pub fn jit_push_inlined_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        base: u32,
        pc: u32,
        nresults: i32,
    ) {
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Lua(Frame {
                closure: cl,
                base,
                pc,
                // Lua call ABI: callee R[0] sits at caller R[A+1], so
                // callee.base = caller.base + A + 1; func_slot is
                // caller.base + A = callee.base - 1.
                func_slot: base - 1,
                n_varargs: 0,
                nresults,
                hook_oldpc: u32::MAX,
                from_c: false,
                tm: None,
                is_hook: false,
                tailcalls: 0,
                ccmt: 0,
            }),
        );
    }

    /// Toggle precompiled-chunk loading. Default `true`. Sandbox embedders
    /// should set to `false` so `load`/`loadstring` reject bytecode input
    /// (which bypasses parser limits and could exploit verifier gaps).
    pub fn set_bytecode_loading(&mut self, enabled: bool) {
        self.bytecode_loading = enabled;
    }

    /// Current bytecode-loading gate state.
    pub fn bytecode_loading(&self) -> bool {
        self.bytecode_loading
    }

    /// Toggle PUC `.luac` bytecode loading. Default `false` — PUC
    /// bytecode is a strictly larger trust surface than luna's own dump
    /// format (third-party toolchain bugs, malformed chunks, unknown
    /// opcode shapes). Enable only for trusted PUC chunks. Per-dialect
    /// translators live in `crate::vm::dump::puc`.
    pub fn set_puc_bytecode_loading(&mut self, enabled: bool) {
        self.puc_bytecode_loading = enabled;
    }

    /// Current PUC bytecode-loading gate state.
    pub fn puc_bytecode_loading(&self) -> bool {
        self.puc_bytecode_loading
    }

    /// Default loader input budget — 256 MiB.
    ///
    /// `Vm::load` and the Lua-level `load(reader, ...)` both refuse
    /// sources whose byte length crosses this cap, returning the
    /// PUC-shaped `not enough memory` error rather than letting the
    /// host allocator try (and crash) to hold the next chunk.
    pub const DEFAULT_LOADER_INPUT_BUDGET: usize = 256 * 1024 * 1024;

    /// Set the loader input byte budget (see
    /// [`Vm::DEFAULT_LOADER_INPUT_BUDGET`]). Pass `usize::MAX` to
    /// effectively disable. Smaller caps are honored verbatim — a 0
    /// cap rejects every non-empty source.
    pub fn set_loader_input_budget(&mut self, bytes: usize) {
        self.loader_input_budget = bytes;
    }

    /// Current loader input byte budget.
    pub fn loader_input_budget(&self) -> usize {
        self.loader_input_budget
    }

    /// Take the error traceback captured at the latest error point and
    /// reset it. Embedders should call this immediately after a failed
    /// `call_value`/`eval`/`call`/etc. — the next public `call_value`
    /// entry clears it. Returns `None` if no error was in flight.
    pub fn take_error_traceback(&mut self) -> Option<String> {
        let levels = self.error_traceback.take()?;
        let tb = crate::vm::callstack::traceback_from_lines(self.version, &levels, 0);
        Some(String::from_utf8_lossy(&tb).into_owned())
    }

    /// PUC `luaL_traceback(L, L, msg, level)` on the running thread: `msg`
    /// (when given) and a newline, then `stack traceback:` and one line per
    /// stack level from `level` on, level 0 being the running function (the
    /// native calling this, when a native does).
    pub fn traceback(&mut self, msg: Option<&[u8]>, level: i64) -> Vec<u8> {
        let mut out = match msg {
            Some(m) => {
                let mut out = m.to_vec();
                out.push(b'\n');
                out
            }
            None => Vec::new(),
        };
        out.extend_from_slice(b"stack traceback:");
        out.extend(self.traceback_lines(None, level));
        out
    }

    /// Arm the soft memory cap. The run loop checks the
    /// heap's tracked byte usage between dispatch turns; on overshoot it
    /// first runs a full collect, and if `bytes` still exceeds the cap it
    /// raises a catchable `"memory cap exceeded"` Lua error and disarms
    /// itself (fire-once: re-arm before the next `call_value` if reusing
    /// the Vm across requests). `None` removes the cap. The accounting is
    /// approximate — internal Vec/Box capacity overhead is not tracked,
    /// so embedders should size the cap with ~2× margin over the desired
    /// hard limit and additionally bound the Vm's lifetime (drop after
    /// each request).
    pub fn set_memory_cap(&mut self, cap: Option<usize>) {
        self.heap.mem_cap = cap;
        self.trap = true;
    }

    /// Approximate bytes the heap is currently holding. Object shells plus
    /// every table's internal array/hash boxes (tracked via
    /// `Heap::apply_bytes_delta` in `set`/`rehash`/`ensure_*`). Proto
    /// bytecode and closure upvalue slices still go uncounted — this is a
    /// lower bound, not a precise `malloc_stats`-style total.
    pub fn memory_used(&self) -> usize {
        self.heap.bytes()
    }

    /// Read upvalue slot `i` of the native function currently on top of the
    /// dispatch chain (the one whose body is executing). Returns `Value::Nil`
    /// when no native is running. Public so the C ABI trampoline can fetch
    /// the host C function pointer it stashed there at registration time.
    pub fn running_native_upvalue(&self, i: usize) -> Value {
        match self.running_natives.last() {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            Some(a) => unsafe {
                let upvals = &(*a.nc.as_ptr()).upvals;
                upvals.get(i).copied().unwrap_or(Value::Nil)
            },
            None => Value::Nil,
        }
    }

    /// Register a table for finalization if its (just-set) metatable carries a
    /// `__gc` metamethod (PUC luaC_checkfinalizer at setmetatable time — adding
    /// `__gc` to the metatable afterwards does not retroactively register).
    pub(crate) fn check_finalizer(&mut self, t: Gc<Table>) {
        // Tables gained finalizers in 5.2; PUC 5.1 runs `__gc` for userdata only.
        if self.version == crate::version::LuaVersion::Lua51 {
            return;
        }
        if !self.get_mm(Value::Table(t), Mm::Gc).is_nil() {
            self.heap.register_finalizable(t);
        }
    }

    /// Same as [`Self::check_finalizer`] for a userdata. PUC 5.1 attaches the
    /// finalizer to the proxy produced by `newproxy(true)` once its metatable
    /// gains `__gc`. gc.lua's "testing userdata" section sets `__gc` on the
    /// metatable that `newproxy` returned, which then needs to flow through.
    /// Kept available for the future 5.2+ `lua_setmetatable` path (which
    /// would re-check at metatable-set time); luna's only userdata
    /// finalizables today come via `newproxy`, which registers itself.
    #[allow(dead_code)]
    pub(crate) fn check_finalizer_userdata(&mut self, u: Gc<crate::runtime::Userdata>) {
        if !self.get_mm(Value::Userdata(u), Mm::Gc).is_nil() {
            self.heap.register_finalizable_userdata(u);
        }
    }

    /// Run pending `__gc` finalizers (objects the collector resurrected for
    /// finalization). Finalizer errors are swallowed — PUC turns them into a
    /// warning; they must never propagate to the mutator. Reentrancy-guarded.
    fn run_finalizers(&mut self) {
        let _ = self.run_finalizers_or_err();
    }

    fn run_finalizers_or_err(&mut self) -> Result<(), LuaError> {
        if self.gc_finalizing {
            return Ok(());
        }
        let pending = self.heap.take_tobefnz();
        if pending.is_empty() {
            return Ok(());
        }
        self.gc_finalizing = true;
        let mut first_err: Option<LuaError> = None;
        for obj in pending {
            let gc = self.get_mm(obj, Mm::Gc);
            // PUC 5.2+ accepts any non-nil `__gc` at setmetatable time to
            // schedule the object for finalization (`__gc = true` is the
            // canonical placeholder); only call it at finalize time when it
            // is actually a function. gc.lua 5.2 :412 wires up exactly this
            // sentinel and then expects no call.
            let callable = matches!(gc, Value::Closure(_) | Value::Native(_));
            if callable {
                // PUC `GCTM` sets `CIST_FIN` on the new ci so
                // `funcnamefromfinalizer` reports `namewhat = "metamethod"`,
                // `name = "__gc"`. luna threads the same outcome through the
                // generic `pending_tm` slot: the Lua frame born from this
                // call consumes it in `push_frame`. Saved/restored around the
                // call in case the handler is a native (which never pops it).
                // Bare event name; `frame_name` / `c_frame_name` add the
                // `"__"` debug prefix for 5.2/5.3, drop it for 5.4+. Matches
                // the convention used by `__close`, `__index`, …
                let saved_tm = self
                    .pending_tm
                    .replace(crate::runtime::function::FrameTm::Gc);
                // PUC `GCTM` runs the finalizer with `luaD_pcall` and no
                // message handler
                if let Err(e) = self.call_protected(gc, &[obj]) {
                    // PUC 5.1 GCTM raised the finalizer's error to the
                    // explicit `collectgarbage()` caller (`gc.lua 5.1 :255`
                    // baselines on `not pcall(collectgarbage)`). 5.2/5.3
                    // wrapped it in `error in __gc metamethod (msg)` first
                    // (`callGCTM` → `luaG_runerror`) but still raised. 5.4
                    // introduced the warning system and switched to "warn
                    // then continue" — never re-raise, just route the
                    // wrapped message through `warn`. gc.lua 5.5 :378 wires
                    // up `_WARN` capture under the `if T then …` block to
                    // baseline on the same wrapped string.
                    if self.version >= LuaVersion::Lua54 {
                        let inner = self.error_text(&e);
                        let msg = format!("error in __gc metamethod ({inner})");
                        self.emit_warn(msg.as_bytes(), false);
                    } else if first_err.is_none() {
                        let wrapped = if self.version >= LuaVersion::Lua52 {
                            let inner = self.error_text(&e);
                            let msg = format!("error in __gc metamethod ({inner})");
                            let s = Value::Str(self.heap.intern(msg.as_bytes()));
                            LuaError(s)
                        } else {
                            e
                        };
                        first_err = Some(wrapped);
                    }
                }
                self.pending_tm = saved_tm;
            }
        }
        self.gc_finalizing = false;
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Drive one incremental GC step (PUC `collectgarbage("step", n)`).
    /// Crosses up to three phases per call:
    ///   1. Pause      → seed Propagate (`gc_start_propagate`)
    ///   2. Propagate  → drain gray up to `budget`; on exhaustion run atomic
    ///                   (`gc_finish_atomic` → tobefnz populated; finalizers
    ///                   run via `run_finalizers`) and enter Sweep
    ///   3. Sweep      → `gc_sweep_step` up to (residual) `budget`
    /// Returns true when this call completed the cycle's sweep (back to
    /// Pause). The budget is spent generously across phases — a large `n`
    /// can finish a whole cycle in one call (PUC stop-the-world step).
    pub(crate) fn gc_step(&mut self, budget: usize) -> bool {
        // Re-entry guard: never recurse — `run_finalizers` calls Lua code
        // that may hit a safe point and try to step again. Re-entry was OK
        // under STW (collect_garbage had its own guard) but here the
        // intermediate phase state would corrupt.
        if self.gc_finalizing {
            return false;
        }
        if self.heap.gc_phase_is_pause() {
            let (roots, extra) = self.gc_roots();
            self.heap.gc_start_propagate(&roots, &extra);
        }
        if self.heap.gc_phase_is_propagate() {
            if !self.heap.gc_step_propagate(budget) {
                return false;
            }
            self.clear_dead_stack();
            let (roots, extra) = self.gc_roots();
            self.heap.gc_remark(&roots, &extra);
            self.heap.gc_finish_atomic();
            // any __gc scheduled by atomic — run before sweep so a finalizer
            // re-registering `self` re-enters the next cycle, not this sweep
            self.run_finalizers();
        }
        // either we just transitioned, or we entered already in Sweep, or
        // a finalizer started a new cycle (gc_sweep_step is a no-op then)
        self.heap.gc_sweep_step(budget)
    }

    // ---- frames & calls ----

    /// Begin calling stack[func_slot] with `nargs` (None: up to self.top).
    /// Returns true if a Lua frame was pushed (the dispatch loop continues
    /// there), false if a native completed inline.
    fn begin_call(
        &mut self,
        func_slot: u32,
        nargs: Option<u32>,
        nresults: i32,
        from_c: bool,
    ) -> Result<bool, LuaError> {
        let mut nargs = match nargs {
            Some(n) => n,
            None => self.top - (func_slot + 1),
        };
        // Consume `pending_is_tail` at the boundary: a tail-call op sets it
        // only for the immediately-following Lua activation. Native dispatch
        // (or `__call` resolution) below must not let it leak to the next
        // begin_call's frame; restore it just before push_frame for the Lua
        // arm so its meaning is preserved across __call chaining.
        let tailcalls = std::mem::take(&mut self.pending_tailcalls);
        let tail_ccmt = std::mem::take(&mut self.pending_ccmt);
        // resolve __call handlers iteratively (PUC tryfuncTM loop): each handler
        // is inserted before the value so it becomes the first argument, and a
        // chain of `__call` tables resolves down to a real function.
        let mut chain = 0u32;
        loop {
            match self.stack[func_slot as usize] {
                Value::Closure(cl) => {
                    // JIT fast path: if the Proto's body fits
                    // the int-arith whitelist, every arg is `Value::Int`,
                    // and the cached arity matches, skip frame setup and
                    // run the cached native fn in-place.
                    if self.try_jit_call_op(cl, func_slot, nargs, nresults) {
                        self.pending_tailcalls = tailcalls;
                        return Ok(false);
                    }
                    self.pending_tailcalls = tailcalls;
                    self.pending_ccmt = if tailcalls > 0 {
                        tail_ccmt
                    } else {
                        chain as u8
                    };
                    self.push_frame(cl, func_slot, nargs, nresults, from_c)?;
                    // Trace-on-call trigger. The frame
                    // we just pushed is the callee whose body the
                    // recorder will trace. Bump the per-Proto call
                    // counter; once it crosses `CALL_HOT_THRESHOLD`
                    // and no other trace is in flight, snapshot the
                    // callee's register window (R[0..max_stack]) and
                    // begin recording at `pc=0`. This is what unlocks
                    // tracing for functions whose body has no negative
                    // `Op::Jmp` back-edge (`fib`, recursive helpers).
                    //
                    // Gated on `trace_jit_enabled`, so the default
                    // dispatch pays a single not-taken branch.
                    if self.jit.trace_enabled {
                        let proto = cl.proto;
                        let c = proto.call_hot_count.get();
                        if c < u32::MAX / 2 {
                            proto.call_hot_count.set(c + 1);
                        }
                        // Relaxed call-trigger:
                        // `c >= THRESHOLD` (not `c == THRESHOLD`) +
                        // `!already_cached` short-circuit. Lets a
                        // discarded short call-trigger close retry
                        // on the next call (fib(10/15/20/25)
                        // pathology — first capture is base-case
                        // [Lt,Jmp,Return1]; coverage-heuristic
                        // discards; next call gets to record at a
                        // potentially deeper recursion point).
                        // Without `already_cached`, the relaxed
                        // condition would re-record over a cached
                        // trace every call.
                        //
                        // Additionally short-circuit on
                        // `proto.trace_gave_up`: the per-Proto discard
                        // cap force-compiles a partial trace and flips
                        // it. `trace_call_head_settled` stands for
                        // "a trace is cached at pc 0 or recording it was
                        // abandoned", so no call scans `traces`.
                        if c >= self.jit.call_hot_threshold
                            && self.jit.active_trace.is_none()
                            && !proto.trace_gave_up.get()
                            && !proto.trace_call_head_settled.get()
                        {
                            // The new frame is on top: index in
                            // `self.frames` is `len() - 1`.
                            let frame_idx = self.frames.len() - 1;
                            // Snapshot R[0..max_stack] at the callee's
                            // base. `push_frame` resized `self.stack`
                            // to `base + max_stack`, so this window is
                            // guaranteed in-bounds.
                            let f = match &self.frames[frame_idx] {
                                CallFrame::Lua(f) => f,
                                _ => unreachable!("push_frame just pushed a Lua frame"),
                            };
                            let max_stack = cl.proto.max_stack as usize;
                            let base_us = f.base as usize;
                            let mut entry_tags = Vec::with_capacity(max_stack);
                            for i in 0..max_stack {
                                let (tag, _) = self.stack[base_us + i].unpack();
                                entry_tags.push(tag);
                            }
                            self.jit.active_trace =
                                Some(Box::new(crate::jit::trace::TraceRecord::start(
                                    cl.proto, 0, entry_tags, true,
                                )));
                            self.jit.recording_frame_base = frame_idx;
                        }
                    }
                    return Ok(true);
                }
                Value::Native(nc) => {
                    if nc.kind != NativeKind::Plain
                        && let Some(r) = self.begin_special_native(nc, func_slot, nargs, nresults)
                    {
                        return r;
                    }
                    // a native that collects (e.g. `collectgarbage`) roots up to
                    // its own arguments — the caller's live registers all sit
                    // below `func_slot` and stay rooted.
                    self.native_nresults = nresults;
                    self.gc_top = func_slot + nargs + 1;
                    // Push the native onto the running-natives chain BEFORE
                    // firing the call hook so that `debug.getinfo(level)` and
                    // `arg_error` from inside the hook see this native as the
                    // currently-running C function (db.lua :344 reads
                    // `getinfo(2, "f").func` for the just-entered callee).
                    // Popped after the matching return hook fires — even on
                    // error, the pop must happen, so the body is bracketed
                    // through a scope guard.
                    self.running_natives.push(crate::vm::callstack::NativeAct {
                        nc,
                        func_slot,
                        nargs,
                        depth: self.frames.len() as u32,
                        // a tail call resolved its `__call` chain before
                        // calling here and passed the count in tail_ccmt
                        ccmt: tail_ccmt + chain as u8,
                    });
                    // PUC C-call discipline: entering a C function sets
                    // L->top to func + 1 + nargs, so a collect triggered
                    // INSIDE the native (explicit `collectgarbage()`, or
                    // an allocation crossing the GC threshold) roots the
                    // whole caller window up to and including the
                    // arguments. Without this raise the cursor is stale —
                    // parked at some earlier, possibly much lower
                    // safe-point — and the collect frees register-held
                    // values of the native's own caller (use-after-free).
                    // Never lower it: a re-entrant chain
                    // (native → Lua → native) must keep the outermost
                    // window rooted.
                    self.gc_top = self.gc_top.max(func_slot + 1 + nargs);
                    // PUC luaD_precall fires the "call" hook for C functions too.
                    // A yield inside the native (coroutine.yield) propagates an
                    // Err and the matching "return" hook fires on resume instead.
                    if let Err(e) = self.hook_call(true, nargs) {
                        self.running_natives.pop();
                        return Err(e);
                    }
                    // Trap a Rust panic in the native and surface it as
                    // a Lua error rather than letting it unwind through the
                    // VM into the embedder. The VM's internal state may still
                    // be inconsistent after a panic (half-pushed args,
                    // dangling GC references), so embedders that catch this
                    // class of error should drop and re-create the Vm — but
                    // it's still better than tearing the host process down.
                    // `AssertUnwindSafe` is sound because the caller is the
                    // dispatch loop and any half-done state is fenced behind
                    // the immediate Err return below.
                    use std::panic::{AssertUnwindSafe, catch_unwind};
                    let result =
                        match catch_unwind(AssertUnwindSafe(|| (nc.f)(self, func_slot, nargs))) {
                            Ok(r) => r,
                            Err(payload) => {
                                let msg = panic_payload_str(&payload);
                                let s = Value::Str(
                                    self.heap.intern(format!("native panic: {msg}").as_bytes()),
                                );
                                Err(LuaError(s))
                            }
                        };
                    let nret = match result {
                        Ok(n) => n,
                        Err(e) => {
                            // PUC raises with the native still on the stack;
                            // remember it for the handler and traceback of the
                            // error (see `raise_to_handler`)
                            let act = self.running_natives.pop().expect("pushed above");
                            self.note_errored_native(act, e.0);
                            return Err(e);
                        }
                    };
                    // PUC `luaD_poscall` fires the return hook BEFORE moving
                    // results into the function's slot — at that point args
                    // sit at `[func_slot + 1, func_slot + 1 + nargs)` and
                    // results above them at `[func_slot + 1 + nargs, …)`.
                    // luna's `nat_return` has already written the results
                    // into `[func_slot, func_slot + nret)`, so we replay PUC's
                    // layout by copying the results up past the preserved
                    // args, firing the hook (with ftransfer = nargs + 1, so
                    // `getlocal(2, ftransfer..)` reads results), and then
                    // copying back for `finish_results`. db.lua :541 reads
                    // `getinfo("r").ftransfer` + `getlocal` to inspect a
                    // returning native's results this way.
                    if self.hook.ret
                        && !self.in_hook
                        && (self.hook.func.is_some() || self.hook.rust_func.is_some())
                    {
                        let res_dst = func_slot + nargs + 1;
                        let need = (res_dst + nret) as usize;
                        if self.stack.len() < need {
                            self.stack.resize(need, Value::Nil);
                        }
                        for i in (0..nret).rev() {
                            self.stack[(res_dst + i) as usize] =
                                self.stack[(func_slot + i) as usize];
                        }
                        // widen the C-frame's argument window for getlocal
                        if let Some(act) = self.running_natives.last_mut() {
                            act.nargs = nargs + nret;
                        }
                        let hr = self.hook_return(true, nargs + 1, nret);
                        if let Some(act) = self.running_natives.last_mut() {
                            act.nargs = nargs;
                        }
                        // restore results into the slot finish_results expects
                        for i in 0..nret {
                            self.stack[(func_slot + i) as usize] =
                                self.stack[(res_dst + i) as usize];
                        }
                        self.running_natives.pop();
                        hr?;
                    } else {
                        self.running_natives.pop();
                    }
                    self.finish_results(func_slot, nret, nresults);
                    // the native may have allocated; collect with the results as
                    // the live boundary (PUC checks GC after a call returns).
                    self.maybe_collect_garbage(self.top);
                    return Ok(false);
                }
                v => {
                    let mm = self.get_mm(v, Mm::Call);
                    if mm.is_nil() || self.call_mm_unusable(mm) {
                        return Err(self.call_err(v));
                    }
                    chain += 1;
                    // PUC 5.5 dropped the chain cap from `MAXTAGRECUR = 200`
                    // (the value 5.4's `lvm.c` uses) down to `MAXCCMT = 16`,
                    // and the 5.5 test exercises the new tight bound directly
                    // (calls.lua :225 builds a 16-deep chain and expects the
                    // 16th to error). 5.4 calls.lua :194 instead builds a 20-
                    // deep chain and expects it to succeed.
                    let cap = if self.version >= crate::version::LuaVersion::Lua55 {
                        15
                    } else {
                        MAX_CCMT
                    };
                    if chain > cap {
                        return Err(self.rt_err("'__call' chain too long"));
                    }
                    // slots above shift by one; at a call site those are dead
                    // temps of the current frame
                    self.stack.insert(func_slot as usize, mm);
                    if self.top > func_slot {
                        self.top += 1;
                    }
                    nargs += 1;
                }
            }
        }
    }

    /// Up to 5.3 `tryfuncTM` takes one `__call` hop and needs a function
    /// there; anything else is a call error on the original object. 5.4
    /// retries the call with whatever `__call` holds, so chains resolve.
    fn call_mm_unusable(&self, mm: Value) -> bool {
        self.version <= LuaVersion::Lua53 && !matches!(mm, Value::Closure(_) | Value::Native(_))
    }

    fn push_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        from_c: bool,
    ) -> Result<(), LuaError> {
        if func_slot + 256 > MAX_LUA_STACK {
            // PUC `luaD_growstack`: the overflow raises "stack overflow" and
            // leaves ERRORSTACKSIZE's extra slots for the xpcall handler that
            // runs on it; overflowing those is LUA_ERRERR, "error in error
            // handling" (errors.lua :606, cstack.lua :29).
            if self.msgh_depth == 0 {
                return Err(self.rt_err("stack overflow"));
            }
            if func_slot + 256 > MAX_LUA_STACK + ERROR_STACK_EXTRA {
                return Err(self.plain_err("error in error handling"));
            }
        }
        let proto = cl.proto;
        let nparams = proto.num_params as u32;
        // 5.5 vararg layout (PUC luaT_adjustvarargs): the extra args stay on the
        // stack just below the new `base`, so a named vararg can be indexed
        // virtually without allocating a table. Rotate `[p1..pn][e1..em]` to
        // `[e1..em][p1..pn]` so the fixed params land at the new base.
        let n_varargs = if proto.is_vararg {
            nargs.saturating_sub(nparams)
        } else {
            0
        };
        if n_varargs > 0 {
            let s = (func_slot + 1) as usize;
            self.stack[s..s + nargs as usize].rotate_left(nparams as usize);
        }
        let base = func_slot + 1 + n_varargs;
        let need = (base + proto.max_stack as u32) as usize;
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        // the whole window past the kept parameters is cleared: the trace
        // dispatcher compares every register's tag in the window with the
        // trace's entry tags, so a stale value where the recording saw nil
        // would turn the trace away (and 5.1's compiler drops a leading
        // `local x` LoadNil on this promise, as PUC 5.1 does)
        let kept = nargs.saturating_sub(n_varargs).min(nparams);
        // SAFETY: just resized above so `need <= stack.len()`; `base + kept <=
        // need` since `base + nparams <= base + max_stack = need` and `kept <=
        // nparams`. `slice::fill` lowers to a single memset on Copy types.
        unsafe {
            self.stack
                .get_unchecked_mut((base + kept) as usize..need)
                .fill(Value::Nil);
        }
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Lua(Frame {
                closure: cl,
                base,
                pc: 0,
                func_slot,
                nresults,
                hook_oldpc: u32::MAX,
                from_c,
                n_varargs,
                // single-shot consume: `close_slots` sets pending_tm before each
                // handler call; the next Lua frame born is that handler's.
                tm: self.pending_tm.take(),
                // `run_hook` sets `pending_is_hook` before dispatching the user
                // hook so its frame reports `namewhat = "hook"` via getinfo.
                is_hook: std::mem::take(&mut self.pending_is_hook),
                tailcalls: std::mem::take(&mut self.pending_tailcalls),
                ccmt: std::mem::take(&mut self.pending_ccmt),
            }),
        );
        // PUC 5.1 `LUAI_COMPAT_VARARG`: populate the hidden `arg` local with
        // `{ n = n_varargs, [1] = e1, [2] = e2, … }`. The compiler reserved
        // the slot at `base + nparams`; the extras sit just below `base` from
        // the vararg rotate above. 5.1 db.lua :279 reads `arg.n` from a line
        // hook; vararg.lua's contradictory expectations were already going to
        // fail either way (some asserts want `arg == nil`).
        if proto.has_compat_vararg_arg {
            let arg_slot = (base + nparams) as usize;
            let t = self.heap.new_table();
            {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                let tm = unsafe { t.as_mut() };
                for i in 0..n_varargs {
                    let v = self.stack[(base - n_varargs + i) as usize];
                    // bounded by `n_varargs` (≤ MAXUPVAL territory), well
                    // below `MAX_ASIZE`
                    let _ = tm.set_int(&mut self.heap, (i + 1) as i64, v);
                }
                let nk = Value::Str(self.heap.intern(b"n"));
                tm.set(&mut self.heap, nk, Value::Int(n_varargs as i64))
                    .expect("'n' key");
            }
            // once-per-table barrier mirrors SETLIST: t is born BLACK during
            // Propagate and the bulk `set_int`/`set` calls above don't barrier
            self.heap
                .barrier_back(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
            self.stack[arg_slot] = Value::Table(t);
        }
        // PUC luaD_precall fires the "call" hook with the new frame current, so
        // a hook calling debug.getinfo(2) sees the entered function. For a Lua
        // callee, PUC `luaD_hookcall` passes `p->numparams` as ntransfer (only
        // fixed params count — extras already live below `base`).
        // A frame born via OP_TailCall fires "tail call" instead (PUC
        // luaD_pretailcall) and skips the matching "return" hook on exit.
        let is_tail = self
            .frames
            .last()
            .and_then(|f| f.lua())
            .is_some_and(|f| f.tailcalls > 0);
        self.hook_call_with(false, nparams, is_tail)?;
        Ok(())
    }

    /// `pcall(f, ...)` (PUC luaB_pcall): push a continuation frame, then drive
    /// the protected call `f` through the interpreter loop. The protected
    /// function and its arguments already sit at `func_slot+1..`, so calling `f`
    /// at `func_slot+1` lets its results land one slot above the continuation —
    /// the loop head then writes `true` at `func_slot` to form `true, results…`.
    /// Always returns `Ok(true)`: a continuation is now on the stack to be
    /// resolved by the loop (even when `f` is a native that already ran inline).
    fn begin_pcall(&mut self, func_slot: u32, nargs: u32, nresults: i32) -> Result<bool, LuaError> {
        if nargs == 0 {
            // `luaL_checkany` fails here: there is no function to call.
            self.with_native_running(func_slot, nargs, |vm| {
                let a = crate::vm::argcheck::Args::new(func_slot, nargs);
                crate::vm::argcheck::check_any(vm, a, 0).map(drop)
            })?;
        }
        if self.pcall_depth >= MAX_C_DEPTH {
            // raised inside pcall, a C function: no position
            return Err(self.plain_err("C stack overflow"));
        }
        self.pcall_depth += 1;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Pcall,
                func_slot,
                nresults,
            }),
        );
        // call f (slot func_slot+1) with the remaining args, asking for all
        // results; a yield or error inside propagates with the continuation kept
        // on the stack (caught by `unwind` / preserved across a yield).
        self.begin_call(func_slot + 1, Some(nargs - 1), -1, true)?;
        Ok(true)
    }

    /// `xpcall(f, msgh, ...)` (PUC luaB_xpcall): like `begin_pcall`, but the
    /// message handler is stashed in the continuation and the arguments are
    /// shifted down over the handler's slot so `f`'s args are contiguous.
    /// `forward` is false for 5.1's `xpcall`, which passes `f` none of them.
    fn begin_xpcall(
        &mut self,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        forward: bool,
    ) -> Result<bool, LuaError> {
        self.with_native_running(func_slot, nargs, |vm| {
            let a = crate::vm::argcheck::Args::new(func_slot, nargs);
            crate::vm::builtins::xpcall_handler(vm, a).map(drop)
        })?;
        if self.pcall_depth >= MAX_C_DEPTH {
            // raised inside pcall, a C function: no position
            return Err(self.plain_err("C stack overflow"));
        }
        self.pcall_depth += 1;
        // layout: [xpcall@func_slot, f@+1, msgh@+2, a1@+3, ...]. Stash msgh and
        // close its gap so f's args become [f@+1, a1@+2, ...].
        let handler = self.stack[(func_slot + 2) as usize];
        // 5.1: `xpcall (f, err)` takes exactly two parameters — extra
        // arguments are NOT forwarded to `f` (5.2 added forwarding;
        // 5.1 calls f with zero args).
        let nfargs = if forward { nargs - 2 } else { 0 };
        for i in 0..nfargs {
            self.stack[(func_slot + 2 + i) as usize] = self.stack[(func_slot + 3 + i) as usize];
        }
        self.top = func_slot + 2 + nfargs;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Xpcall { handler },
                func_slot,
                nresults,
            }),
        );
        self.begin_call(func_slot + 1, Some(nfargs), -1, true)?;
        Ok(true)
    }

    /// `pairs(t)` where `t` has a `__pairs` metamethod (PUC luaB_pairs's
    /// lua_callk path): drive `__pairs(t)` through the loop with a `Pairs`
    /// continuation so a `coroutine.yield` inside it suspends cleanly. The
    /// metamethod is called in `pairs`'s own slot, so its (≤4, nil-padded)
    /// results land exactly where `pairs`'s results belong.
    /// Run a check of the native at `func_slot` while it counts as the running
    /// C function, so an argument error names it the way PUC does. pcall and
    /// xpcall check their arguments in the dispatcher, before the native
    /// would otherwise be entered.
    fn with_native_running(
        &mut self,
        func_slot: u32,
        nargs: u32,
        check: impl FnOnce(&mut Vm) -> Result<(), LuaError>,
    ) -> Result<(), LuaError> {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("pcall/xpcall dispatch sits on a native")
        };
        self.running_natives.push(crate::vm::callstack::NativeAct {
            nc,
            func_slot,
            nargs,
            depth: self.frames.len() as u32,
            ccmt: 0,
        });
        let r = check(self);
        self.running_natives.pop();
        r
    }

    fn begin_pairs(&mut self, func_slot: u32, nresults: i32) -> Result<bool, LuaError> {
        let arg = self.stack[(func_slot + 1) as usize];
        let mm = self.get_mm(arg, Mm::Pairs);
        // layout becomes [pairs@func_slot, mm@func_slot+1, t@func_slot+2]:
        // `pairs` keeps its slot so the debug interface can report it as the
        // C function running below the metamethod. Call mm(t) wanting 4.
        let need = (func_slot + 3) as usize;
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        self.stack[(func_slot + 2) as usize] = arg;
        self.stack[(func_slot + 1) as usize] = mm;
        self.top = func_slot + 3;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Pairs,
                func_slot,
                nresults,
            }),
        );
        let want = crate::vm::builtins::pairs_mm_results(self) as i32;
        self.begin_call(func_slot + 1, Some(1), want, true)?;
        Ok(true)
    }

    /// The running (top) Lua frame. The interpreter only reads this while a Lua
    /// frame is on top — a continuation frame is never the running frame (it is
    /// consumed the instant the call it protects unwinds onto it).
    #[inline]
    fn top_frame(&self) -> &Frame {
        self.frames
            .last()
            .and_then(CallFrame::lua)
            .expect("running Lua frame")
    }

    #[inline]
    fn top_frame_mut(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .and_then(CallFrame::lua_mut)
            .expect("running Lua frame")
    }

    /// Pad/announce results sitting at func_slot. Results past `wanted`
    /// are cleared; nothing else is: values left higher up by the call are
    /// dead and stay safe to mark (see `clear_dead_stack`), as with PUC's
    /// `moveresults`.
    #[inline]
    pub(crate) fn finish_results(&mut self, func_slot: u32, nret: u32, wanted: i32) {
        if wanted < 0 {
            self.top = func_slot + nret;
            return;
        }
        let wanted = wanted as u32;
        let new_top = func_slot + wanted;
        if nret < wanted {
            self.pad_results(func_slot + nret, new_top);
        } else if nret > wanted {
            self.stack[new_top as usize..(func_slot + nret) as usize].fill(Value::Nil);
        }
        self.top = new_top;
    }

    /// Nil the missing results `[from, to)`.
    fn pad_results(&mut self, from: u32, to: u32) {
        if self.stack.len() < to as usize {
            self.stack.resize(to as usize, Value::Nil);
        }
        self.stack[from as usize..to as usize].fill(Value::Nil);
    }

    /// Current Lua call-frame depth (read-only).
    /// Used by `EvalFuture` on the bootstrap poll to compute the
    /// `entry_depth` it will pass to subsequent resume slices.
    pub(crate) fn frame_count(&self) -> usize {
        self.frames.len()
    }

    fn take_results(&mut self, func_slot: u32) -> Vec<Value> {
        let nret = self.top - func_slot;
        let out = self.stack[func_slot as usize..(func_slot + nret) as usize].to_vec();
        self.stack.truncate(func_slot as usize);
        self.top = func_slot;
        out
    }

    // ---- open upvalues ----

    #[doc(hidden)]
    pub fn find_or_create_upval(&mut self, slot: u32) -> Gc<Upvalue> {
        match self.open_upvals.binary_search_by_key(&slot, |&(s, _)| s) {
            Ok(i) => self.open_upvals[i].1,
            Err(i) => {
                let uv = self.heap.new_upvalue(UpvalState::Open {
                    slot,
                    thread: self.current,
                });
                self.open_upvals.insert(i, (slot, uv));
                uv
            }
        }
    }

    pub(crate) fn close_from(&mut self, slot: u32) {
        while let Some(&(s, uv)) = self.open_upvals.last() {
            if s < slot {
                break;
            }
            let v = self.stack[s as usize];
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { uv.as_mut() }.set_closed(v);
            self.heap
                .barrier_forward(uv.as_ptr() as *mut crate::runtime::heap::GcHeader, v);
            self.open_upvals.pop();
        }
    }

    /// Register a to-be-closed slot (TBC op / generic-for closing value).
    fn register_tbc(&mut self, slot: u32) -> Result<(), LuaError> {
        let v = self.stack[slot as usize];
        if matches!(v, Value::Nil | Value::Bool(false)) {
            return Ok(()); // nil and false are silently ignored
        }
        if self.get_mm(v, Mm::Close).is_nil() {
            // PUC `checkclosemth`: "variable '<name>' got a non-closable
            // value", the name as `luaG_findlocal` gives it — the frame's
            // locvars at this pc, else "(temporary)".
            let f = self.top_frame();
            let reg = slot - f.base;
            let pc = (f.pc as usize).saturating_sub(1);
            let name = crate::vm::objname::getlocalname(&f.closure.proto, reg, pc)
                .unwrap_or("(temporary)");
            return Err(self.rt_err(&format!("variable '{name}' got a non-closable value")));
        }
        // compiled code registers in register order and closes before it
        // registers a slot again; only a crafted chunk (`TBC R0; TBC R0`)
        // breaks that, which PUC's list cannot represent either
        if self.tbc.last().is_some_and(|&s| s >= slot) {
            return Err(self.rt_err("'<close>' state corrupted"));
        }
        self.tbc.push(slot);
        Ok(())
    }

    /// Close upvalues and run `__close` handlers for slots ≥ `from`
    /// (handlers in reverse registration order; PUC luaF_close).
    fn close_slots(&mut self, from: u32, err: Option<Value>) -> Result<(), LuaError> {
        self.close_from(from);
        // PUC: handlers run in reverse declaration order; an error raised by a
        // handler becomes the error object passed to the remaining ones, and
        // the rest are still closed. The last raised error propagates.
        let mut pending = err;
        let mut result = Ok(());
        let saved_err = self.closing_err;
        // On a normal close the handler runs within the closing function's
        // activation (debug parent = that function); during error unwinding the
        // function's frame is already gone, so the handler sits at the C
        // boundary instead (PUC: luaF_close runs after the ci is restored).
        let error_close = err.is_some();
        while let Some(&s) = self.tbc.last() {
            if s < from {
                break;
            }
            self.tbc.pop();
            let v = self.stack[s as usize];
            if matches!(v, Value::Nil | Value::Bool(false)) {
                continue;
            }
            let mm = self.get_mm(v, Mm::Close);
            if mm.is_nil() {
                // PUC `prepclosingmethod`: the __close metamethod was present
                // at OP_TBC (else we would have errored there) but has since
                // been removed/replaced. Treat as a non-callable target.
                let tn = self.obj_typename(v);
                let e = self.rt_err(&format!(
                    "attempt to call a {tn} value (metamethod 'close')"
                ));
                pending = Some(e.0);
                result = Err(e);
                continue;
            }
            // root the pending error: a handler may trigger a collection
            self.closing_err = pending;
            // PUC `luaF_close` sets `ci->u.l.tm = TM_CLOSE` so traceback /
            // getinfo report the handler as "in metamethod 'close'". Saved/
            // restored around the call to cover the path where `mm` is a
            // native (`push_frame` never consumes it) or it raises before
            // reaching push_frame.
            let saved_tm = self
                .pending_tm
                .replace(crate::runtime::function::FrameTm::Close);
            // PUC 5.4 `prepclosingmethod` always pushed (obj, errobj) — errobj
            // is nil on a normal close (5.4 locals.lua :875's
            // `func2close(coroutine.yield)` wrap pins `(self, nil)` back
            // through the yield). PUC 5.5 dropped the trailing nil: a clean
            // close passes only `obj`, the error case still passes both
            // (5.5 locals.lua :314 `select("#", ...) == n` with n=1 for the
            // normal-close arms, n=2 for the error arm).
            let call = match pending {
                Some(e) => self.call_value_impl(mm, &[v, e], error_close),
                None => {
                    if self.version >= LuaVersion::Lua55 {
                        self.call_value_impl(mm, &[v], error_close)
                    } else {
                        self.call_value_impl(mm, &[v, Value::Nil], error_close)
                    }
                }
            };
            self.pending_tm = saved_tm;
            if let Err(e) = call {
                pending = Some(e.0);
                result = Err(e);
            }
        }
        self.closing_err = saved_err;
        result
    }

    /// Yieldable variant of `close_slots`: drive the chain of `__close`
    /// handlers for slots ≥ `from` through the interpreter loop with a
    /// `Cont::Close` continuation, so a `coroutine.yield()` inside any handler
    /// suspends cleanly (the close iteration's state rides on the thread's
    /// frame/stack like any other suspended call) — PUC's `lua_callk` pattern
    /// applied to `luaF_close`. `after` runs when every slot is closed; if
    /// `after` is `Return` and we've returned past `entry_depth`,
    /// `Ok(Some(vals))` carries the result up to the host caller.
    fn begin_close(
        &mut self,
        from: u32,
        err: Option<Value>,
        after: AfterClose,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        self.close_from(from);
        self.drive_close(from, err, after, entry_depth)
    }

    /// Pop tbc slots ≥ `from`, skipping nil/false and synthesising a
    /// non-callable-mm error for an `__close` that was reset to a bad value
    /// between OP_TBC and now (PUC `prepclosingmethod`). The first real
    /// handler pushes a `Cont::Close` + `begin_call` and returns `Ok(None)`;
    /// the interpreter then drives the handler and re-enters this driver via
    /// the `Cont::Close` consumer in `run()`. When the chain is exhausted,
    /// the threaded error (if any) propagates or `after` fires.
    fn drive_close(
        &mut self,
        from: u32,
        mut pending: Option<Value>,
        after: AfterClose,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        loop {
            let drained = match self.tbc.last() {
                None => true,
                Some(&s) => s < from,
            };
            if drained {
                return self.finish_close_after(after, pending, entry_depth);
            }
            let s = self.tbc.pop().expect("tbc non-empty");
            let v = self.stack[s as usize];
            if matches!(v, Value::Nil | Value::Bool(false)) {
                continue;
            }
            let mm = self.get_mm(v, Mm::Close);
            if mm.is_nil() {
                let tn = self.obj_typename(v);
                let e = self.rt_err(&format!(
                    "attempt to call a {tn} value (metamethod 'close')"
                ));
                pending = Some(e.0);
                continue;
            }
            // A real handler: stage [mm, v, (err?)] above the current top,
            // record the close iteration state in a Cont::Close, and let the
            // interpreter dispatch the handler. On return the run() head
            // re-enters this driver via the Cont::Close consumer.
            let func_slot = self.top;
            let error_close = pending.is_some();
            let need = (func_slot + 3) as usize;
            if self.stack.len() < need {
                self.stack.resize(need, Value::Nil);
            }
            self.stack[func_slot as usize] = mm;
            self.stack[func_slot as usize + 1] = v;
            // PUC 5.4 always passes (obj, errobj=nil) on a normal close;
            // 5.5 drops the trailing nil. 5.4 locals.lua :875 vs 5.5 :314.
            let nargs = match pending {
                Some(e) => {
                    self.stack[func_slot as usize + 2] = e;
                    2u32
                }
                None => {
                    if self.version >= LuaVersion::Lua55 {
                        1u32
                    } else {
                        self.stack[func_slot as usize + 2] = Value::Nil;
                        2u32
                    }
                }
            };
            self.top = func_slot + 1 + nargs;
            // Root the pending error during the call (a handler may collect).
            let saved_err = self.closing_err;
            self.closing_err = pending;
            // PUC `luaF_close` flags the handler frame as "metamethod 'close'"
            // for traceback / getinfo.
            let saved_tm = self
                .pending_tm
                .replace(crate::runtime::function::FrameTm::Close);
            frames_push_sync(
                &mut self.frames,
                &mut self.frames_top,
                &mut self.trap,
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Close(CloseCont {
                        from,
                        pending,
                        after,
                    }),
                    func_slot,
                    nresults: 0,
                }),
            );
            // PUC luaF_close runs a normal close *within* the closing
            // function's activation (debug parent = that function); during an
            // error unwind the function's frame is already gone and the
            // handler sits at the C boundary instead.
            let r = self.begin_call(func_slot, Some(nargs), 0, error_close);
            self.pending_tm = saved_tm;
            self.closing_err = saved_err;
            r?;
            return Ok(None);
        }
    }

    /// Fire `after` once every `__close` handler has run. `Block` propagates
    /// any remaining error or simply continues; `Return` performs OP_Return's
    /// tail (hook + frame pop + result delivery) and may surface results to
    /// the host when the function whose return triggered the close was the
    /// entry activation, but only on a clean drain — a pending error skips
    /// the return tail and propagates instead. `ResumeUnwind` pops the
    /// deferred Lua frame and re-raises, letting a handler's own error win
    /// over the original propagating one (PUC luaF_close).
    fn finish_close_after(
        &mut self,
        after: AfterClose,
        pending: Option<Value>,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        match after {
            AfterClose::Block => match pending {
                Some(e) => Err(LuaError(e)),
                None => Ok(None),
            },
            AfterClose::Return {
                abs_a,
                nret,
                from_native,
            } => match pending {
                Some(e) => Err(LuaError(e)),
                None => self.complete_return(abs_a, nret, from_native, entry_depth),
            },
            AfterClose::ResumeUnwind { func_slot, err } => {
                // The aborting Lua frame was popped before `begin_close`;
                // restore the catcher's stack window down to `func_slot` and
                // re-raise — preferring a handler-raised error over the
                // original (PUC luaF_close).
                self.stack.truncate(func_slot as usize);
                self.top = func_slot;
                self.tbc.retain(|&s| s < func_slot);
                Err(LuaError(pending.unwrap_or(err)))
            }
        }
    }

    /// OP_Return's post-close tail: fire the "return" hook (frame still
    /// current), pop the Lua frame, slide results into `func_slot`, then
    /// either hand them to the host (`Ok(Some(vals))` when we've returned
    /// past `entry_depth`), leave them contiguous for an exposed
    /// pcall/xpcall continuation, or finish into the caller's expected
    /// result slot. Mirrors the synchronous OP_Return tail so both paths
    /// share semantics — the `from_native` flag selects the right "return"
    /// hook context for `hook_return`.
    fn complete_return(
        &mut self,
        abs_a: u32,
        nret: u32,
        from_native: bool,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        // ftransfer is the local index (1-based) of the first result, as
        // `getinfo("r").ftransfer + getlocal(level, k)` consumes it. luna
        // exposes locals starting at `frame.base` (= func_slot + 1 +
        // n_varargs for a vararg call), so the conversion is the absolute
        // result slot minus base, plus one to make it 1-based. db.lua 5.4
        // :542 (`foo1(); on=false; eqseq(out, {10, 0})`) pins the vararg
        // shape end-to-end.
        let ftransfer = self
            .frames
            .last()
            .and_then(CallFrame::lua)
            .map(|fr| {
                let raw = abs_a.saturating_sub(fr.base) + 1;
                // 5.5 anonymous-vararg functions get a `(vararg table)` pseudo
                // local injected at index `numparams + 1`, so getlocal
                // numbering shifts results past it (5.5 db.lua :539
                // `eqseq(out, {10, 0})`). 5.4 and earlier have no such pseudo.
                if fr.closure.proto.has_vararg_table_pseudo {
                    raw + 1
                } else {
                    raw
                }
            })
            .unwrap_or(1);
        // PUC 5.1 `luaD_poscall`: fire one extra "tail return" hook event
        // per tail call that collapsed into this activation, *after* its
        // own "return". `tailcalls` tracks that count exactly (PUC
        // `ci->u.l.tailcalls`). 5.2+ retired LUA_HOOKTAILRET, so the
        // "return" hook fires once even when the activation absorbed
        // multiple tail calls — only `istailcall` on getinfo surfaces the
        // collapse. 5.1 db.lua :366 pins the event ordering.
        let tailcalls = if self.version <= LuaVersion::Lua51 {
            self.frames
                .last()
                .and_then(|f| f.lua())
                .map(|f| f.tailcalls)
                .unwrap_or(0)
        } else {
            0
        };
        self.hook_return(from_native, ftransfer, nret)?;
        for _ in 0..tailcalls {
            self.hook_tail_return()?;
        }
        let CallFrame::Lua(fr) =
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap)
                .expect("no frame")
        else {
            unreachable!("returning from a non-Lua frame")
        };
        for i in 0..nret {
            self.stack[(fr.func_slot + i) as usize] = self.stack[(abs_a + i) as usize];
        }
        if self.frames.len() < entry_depth {
            self.top = fr.func_slot + nret;
            return Ok(Some(self.take_results(fr.func_slot)));
        } else if matches!(self.frames.last(), Some(CallFrame::Cont(_))) {
            self.top = fr.func_slot + nret;
        } else {
            self.finish_results(fr.func_slot, nret, fr.nresults);
        }
        Ok(None)
    }

    /// Return0 / Return1 without the close and hook machinery (PUC
    /// `OP_RETURN0` / `OP_RETURN1`): when no return hook can fire, nothing
    /// in this frame needs closing and the caller is a Lua frame or a
    /// metamethod's continuation inside this activation, the return is the
    /// pop, the result copy and the result count that `complete_return`
    /// would do. Returns `false`, having done nothing, otherwise.
    #[inline]
    fn return_to_lua(&mut self, base: u32, abs_a: u32, nret: u32, entry_depth: usize) -> bool {
        let n = self.frames.len();
        if self.hook.ret && self.hook_armed()
            || self.open_upvals.last().is_some_and(|&(s, _)| s >= base)
            || self.tbc.last().is_some_and(|&s| s >= base)
            || n <= entry_depth
            || n < 2
        {
            return false;
        }
        let to_meta = match &self.frames[n - 2] {
            CallFrame::Lua(_) => false,
            CallFrame::Cont(c) if matches!(c.kind, ContKind::Meta(_)) => true,
            CallFrame::Cont(_) => return false,
        };
        let Some(CallFrame::Lua(fr)) =
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap)
        else {
            unreachable!("returning from a non-Lua frame")
        };
        for i in 0..nret {
            self.stack[(fr.func_slot + i) as usize] = self.stack[(abs_a + i) as usize];
        }
        if to_meta {
            self.top = fr.func_slot + nret;
        } else {
            self.finish_results(fr.func_slot, nret, fr.nresults);
        }
        true
    }

    #[doc(hidden)]
    pub fn upval_get(&self, cl: Gc<LuaClosure>, idx: u32) -> Value {
        match cl.upvals()[idx as usize].state() {
            UpvalState::Open { slot, thread } => self.read_slot(slot, thread),
            UpvalState::Closed(v) => v,
        }
    }

    fn upval_set(&mut self, cl: Gc<LuaClosure>, idx: u32, v: Value) {
        let uv = cl.upvals()[idx as usize];
        match uv.state() {
            UpvalState::Open { slot, thread } => self.write_slot(slot, thread, v),
            UpvalState::Closed(_) => {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { uv.as_mut() }.set_closed(v);
                // forward barrier: a closed upvalue is single-slot, so the
                // forward variant is cheaper than barrier_back (PUC uses
                // `luaC_barrier_` for upvalues; `luaC_barrierback_` for
                // tables / threads).
                self.heap
                    .barrier_forward(uv.as_ptr() as *mut crate::runtime::heap::GcHeader, v);
            }
        }
    }

    // ---- register / error helpers ----

    #[inline(always)]
    fn r(&self, base: u32, i: u32) -> Value {
        // SAFETY: the compiler reserves `proto.max_stack` slots above `base`
        // at frame entry (`push_frame` sizes the stack up to base + max_stack),
        // and every bytecode-generated reference falls within `[0, max_stack)`.
        // PUC's vmfetch uses raw `R(A)` (`s2v(L->base + A)`) for the same
        // reason. The bounds check would re-validate this invariant on every
        // op — the dispatch hot path can't afford it.
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { *self.stack.get_unchecked((base + i) as usize) }
    }

    #[inline(always)]
    fn set_r(&mut self, base: u32, i: u32, v: Value) {
        // SAFETY: see `r` — `base + i < base + max_stack <= stack.len()` by
        // frame-entry contract.
        unsafe {
            *self.stack.get_unchecked_mut((base + i) as usize) = v;
        }
    }

    #[doc(hidden)]
    pub fn rt_err(&mut self, msg: &str) -> LuaError {
        let text = match self.position_prefix() {
            Some(p) => format!("{p}{msg}"),
            None => msg.to_string(),
        };
        LuaError(Value::Str(self.heap.intern(text.as_bytes())))
    }

    /// Error without the `chunk:line:` position prefix. PUC's
    /// `resume_error` (ldo.c) pushes its message as a bare literal,
    /// so `cannot resume dead coroutine` etc. must not be prefixed.
    pub(crate) fn plain_err(&mut self, msg: &str) -> LuaError {
        LuaError(Value::Str(self.heap.intern(msg.as_bytes())))
    }

    /// A string a library built from pieces of any size: one longer than a
    /// string can hold raises, as the concatenation operator does.
    pub(crate) fn built_str(&mut self, bytes: &[u8]) -> Result<Value, LuaError> {
        if bytes.len() > crate::runtime::string::MAX_LEN {
            return Err(self.rt_err("string length overflow"));
        }
        Ok(Value::Str(self.heap.intern(bytes)))
    }

    pub(crate) fn type_err(&mut self, what: &str, v: Value) -> LuaError {
        let extra = self.subject_varinfo(v);
        let tn = self.obj_typename(v);
        let msg = self.compose_type_err(what, &tn, &extra);
        self.runerror(&msg)
    }

    /// Assemble a `luaG_typeerror` / `luaG_callerror` message in the dialect's
    /// word order.
    ///
    /// PUC ≤5.2 names the operand first — `attempt to call field 'f' (a nil
    /// value)`. 5.3 flipped it to type-first — `attempt to call a nil value
    /// (field 'f')`. luna emitted the 5.3+ form on every dialect, so every
    /// such error was worded wrong under 5.1/5.2.
    ///
    /// Two shapes carry no operand name on ≤5.2 and must collapse to the bare
    /// message: an absent varinfo (identical across dialects), and a
    /// metamethod target — ≤5.2's `luaG_typeerror` only names locals, globals,
    /// fields, upvalues and methods, so `(metamethod 'add')` has no ≤5.2
    /// counterpart and is dropped rather than reworded. All four shapes were
    /// measured against stock 5.1.5 / 5.2.4 / 5.5.1 before this was written.
    fn compose_type_err(&self, what: &str, tn: &str, extra: &str) -> String {
        if self.version() > crate::version::LuaVersion::Lua52 {
            return format!("attempt to {what} a {tn} value{extra}");
        }
        // `extra` is "" or " (kind 'name')" — unwrap to "kind 'name'".
        let inner = extra
            .trim_start()
            .trim_start_matches('(')
            .trim_end_matches(')');
        if inner.is_empty() || inner.starts_with("metamethod") {
            format!("attempt to {what} a {tn} value")
        } else {
            format!("attempt to {what} {inner} (a {tn} value)")
        }
    }

    /// Name the offending operand of the current instruction (PUC varinfo) for
    /// a type error, e.g. " (global 'x')". The faulting value `bad` is matched
    /// to the instruction's subject register(s); a native-raised error whose
    /// current instruction doesn't hold `bad` simply yields "".
    fn subject_varinfo(&self, bad: Value) -> String {
        use crate::vm::isa::Op;
        // PUC `varinfo` names a variable only for a Lua activation
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.last().and_then(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        let mut cands: Vec<u32> = Vec::new();
        match instr.op() {
            // indexed reads / length / method: the table/object is in B
            Op::GetField | Op::GetI | Op::GetTable | Op::SelfOp | Op::Len => {
                cands.push(instr.b());
            }
            // indexed writes / calls: the table/function is in A
            Op::SetField | Op::SetI | Op::SetTable | Op::Call | Op::TailCall => {
                cands.push(instr.a());
            }
            // arithmetic/bitwise: a register operand (B, and C unless constant)
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Mod
            | Op::Pow
            | Op::IDiv
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr => {
                cands.push(instr.b());
                if !instr.k() {
                    cands.push(instr.c());
                }
            }
            Op::Unm | Op::BNot => cands.push(instr.b()),
            // arithmetic on a constant or an immediate: the register operand
            Op::AddI
            | Op::SubI
            | Op::AddK
            | Op::SubK
            | Op::MulK
            | Op::ModK
            | Op::PowK
            | Op::DivK
            | Op::IDivK
            | Op::BAndK
            | Op::BOrK
            | Op::BXorK
            | Op::ShrI
            | Op::ShlI => cands.push(instr.b()),
            // indexing an upvalue table (`_ENV` for a global): PUC
            // `getupvalname` finds the value among the closure's upvalues
            Op::GetTabUp | Op::SetTabUp => {
                let u = if instr.op() == Op::GetTabUp {
                    instr.b()
                } else {
                    instr.a()
                };
                if self.upval_get(f.closure, u).raw_eq(bad)
                    && let Some(d) = p.upvals.get(u as usize)
                {
                    return format!(" (upvalue '{}')", d.name);
                }
            }
            Op::Concat => {
                let a = instr.a();
                for r in a..a + instr.b() {
                    cands.push(r);
                }
            }
            _ => {}
        }
        // Up to 5.3 a binary operator takes a constant operand straight
        // from the constant table (RK), where `varinfo` cannot see it, so a
        // string constant is not named there; unary operators load it into
        // a register first and do name it.
        let rk_operands = self.version <= LuaVersion::Lua53
            && matches!(
                instr.source_op(),
                Op::Add
                    | Op::Sub
                    | Op::Mul
                    | Op::Div
                    | Op::Mod
                    | Op::Pow
                    | Op::IDiv
                    | Op::BAnd
                    | Op::BOr
                    | Op::BXor
                    | Op::Shl
                    | Op::Shr
            );
        for reg in cands {
            if self.r(f.base, reg).raw_eq(bad) {
                return match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                    Some(("constant", _)) if rk_operands => String::new(),
                    Some((kind, name)) => format!(" ({kind} '{name}')"),
                    None => String::new(),
                };
            }
        }
        String::new()
    }

    /// "attempt to call a X value", enriched (PUC luaG_callerror) with a name
    /// for the call target: "(global 'f')" for a direct call, or "(metamethod
    /// 'add')" when the call is a metamethod dispatched by the current opcode.
    fn call_err(&mut self, v: Value) -> LuaError {
        let extra = self.call_target_varinfo(v);
        let tn = self.obj_typename(v);
        let msg = self.compose_type_err("call", &tn, &extra);
        self.runerror(&msg)
    }

    /// Name the offending call target. A metamethod dispatch pushes a `Cont`
    /// frame before the call, so the opcode that triggered it lives in the
    /// nearest *Lua* frame — read that instruction: OP_CALL names the function
    /// register, any metamethod-bearing opcode yields "(metamethod 'event')".
    fn call_target_varinfo(&self, bad: Value) -> String {
        use crate::vm::isa::Op;
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.iter().rev().find_map(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        match instr.source_op() {
            Op::Call | Op::TailCall => {
                let reg = instr.a();
                if self.r(f.base, reg).raw_eq(bad) {
                    match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                        Some((kind, name)) => format!(" ({kind} '{name}')"),
                        None => String::new(),
                    }
                } else {
                    String::new()
                }
            }
            // 5.4 `funcnamefromcode` names the generic-for iterator call
            // (5.3 had the entry but raised through plain `luaG_typeerror`)
            Op::TForCall if self.version >= LuaVersion::Lua54 => {
                " (for iterator 'for iterator')".to_string()
            }
            // 5.4 `funcnamefromcall` names the metamethod; up to 5.3 the
            // call raised through `luaG_typeerror`, whose `varinfo` does not
            op if self.version >= LuaVersion::Lua54 => match mm_event_name(op) {
                Some(ev) => format!(" (metamethod '{ev}')"),
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    /// "number has no integer representation", enriched (PUC luaG_tointerror)
    /// with a "(field 'x')"-style suffix naming the offending operand of the
    /// current arithmetic instruction when it can be recovered from bytecode.
    fn no_int_rep_err(&mut self) -> LuaError {
        let extra = self.bad_operand_varinfo();
        self.runerror(&format!("number{extra} has no integer representation"))
    }

    /// Inspect the current frame's faulting instruction: find the register
    /// operand holding a float with no integer representation and name it.
    fn bad_operand_varinfo(&self) -> String {
        if self.native_on_top() {
            return String::new();
        }
        let Some(f) = self.frames.last().and_then(CallFrame::lua) else {
            return String::new();
        };
        let proto = f.closure.proto;
        let p: &crate::runtime::Proto = &proto;
        let pc = f.pc as usize;
        if pc == 0 || pc > p.code.len() {
            return String::new();
        }
        let instr = p.code[pc - 1];
        let mut regs = vec![instr.b()];
        // C of a constant- or immediate-operand opcode is not a register
        if !instr.k() && instr.arith_const_op().is_none() {
            regs.push(instr.c());
        }
        let no_int = |n: Option<Num>| matches!(n, Some(Num::Float(x)) if crate::runtime::value::f2i_exact(x).is_none());
        for reg in regs {
            let v = self.r(f.base, reg);
            // before 5.4 a numeric string is converted first, so "2.5" is
            // the operand without an integer value
            let n = self.arith_operand()(v);
            if no_int(n) {
                return match crate::vm::objname::getobjname_in(p, pc - 1, reg, self.version) {
                    Some((kind, name)) => format!(" ({kind} '{name}')"),
                    None => String::new(),
                };
            }
        }
        String::new()
    }

    /// Position prefix of the currently executing Lua frame. PUC `luaL_error`
    /// calls `luaL_where(L, 1)` which reads `L->ci->previous`. When the prior
    /// frame is a C function (e.g. a pcall Cont parked above `require`'s
    /// native call), PUC pushes no prefix — match that by looking only at the
    /// topmost frame directly and bailing if it is anything but a Lua frame.
    pub(crate) fn position_prefix(&self) -> Option<String> {
        let f = match self.frames.last()? {
            CallFrame::Lua(f) => f,
            // a native metamethod runs above the Meta continuation of the
            // instruction that triggered it: that Lua function is its caller
            CallFrame::Cont(NativeCont {
                kind: ContKind::Meta(_),
                ..
            }) => self.frames.iter().rev().nth(1)?.lua()?,
            CallFrame::Cont(_) => return None,
        };
        let proto = f.closure.proto;
        // a stripped chunk: no source in luna's own format, no line info in
        // PUC's (whose loader names the missing source "=?")
        if proto.source.as_bytes().is_empty() || proto.lines.is_empty() {
            return Some(self.stripped_prefix());
        }
        let line = proto.lines[(f.pc as usize).saturating_sub(1).min(proto.lines.len() - 1)];
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let raw = unsafe { crate::runtime::string::bytes_of(proto.source.as_ptr()) };
        let display = crate::vm::lib_debug::chunk_id(self.version, raw);
        let src = String::from_utf8_lossy(&display).into_owned();
        Some(format!("{src}:{line}: "))
    }

    /// PUC `luaG_addinfo` prefix for a stripped chunk. 5.5 substitutes "=?"
    /// for the source and renders the line as "?" (so the prefix reads
    /// `?:?: `). 5.4 and below leave the source NULL ("?") and use the raw
    /// `getfuncline = -1`, so the prefix reads `?:-1: ` (5.4 errors.lua :282
    /// matches `^%?:%-1:`).
    fn stripped_prefix(&self) -> String {
        if self.version >= crate::version::LuaVersion::Lua55 {
            "?:?: ".to_string()
        } else {
            "?:-1: ".to_string()
        }
    }

    /// PUC `luaL_where(L, level)`: `"short_src:line: "` for the function at
    /// `level` (0 = the running native), or `None` when that level does not
    /// exist or has no line information (a C function, a stripped chunk).
    pub(crate) fn position_prefix_at_level(&self, level: i64) -> Option<String> {
        let ts = self.thread_stack(None);
        let i = usize::try_from(level).ok()?;
        if i >= ts.levels.len() {
            return None;
        }
        let line = ts.currentline(i);
        if line <= 0 {
            return None;
        }
        let DbgKind::Lua(fi) = ts.levels[i] else {
            return None;
        };
        let ar = self.closure_ar(ts.lua(fi).closure);
        Some(format!(
            "{}:{line}: ",
            String::from_utf8_lossy(&ar.short_src)
        ))
    }

    // ---- the interpreter ----

    /// Run from the current top frame down to (but not past) `entry_depth`
    /// frames. Coroutine driving passes `entry_depth = 1` so the whole thread
    /// runs to completion or a yield.
    /// Resume the dispatcher from the saved
    /// `entry_depth` (captured pre-yield by `drive_one`). Called by
    /// `EvalFuture::poll` on every poll after the first to walk the
    /// existing call frames until the next `BudgetExhausted` or
    /// terminal `Ok`/`Err`. Not a public-API surface; the
    /// embedder reaches it through `Vm::eval_async`.
    pub(crate) fn exec_with_async(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        self.exec_with(entry_depth)
    }

    fn exec_with(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        loop {
            let r = self.run(entry_depth);
            if r.is_err()
                && (self.yielding.is_some()
                    || self.terminating.is_some()
                    || self.host_yield_pending
                    || self.pending_async_native_fut.is_some())
            {
                // a `coroutine.yield` is in flight: keep the frames intact (they
                // are the suspended coroutine's saved state) and propagate to
                // resume. A self-close termination propagates the same way, so a
                // protecting pcall on the way out cannot catch (unwind) it.
                // `host_yield_pending` is the async-mode
                // analogue: the sentinel must reach `drive_one` without
                // a protecting `pcall` swallowing it.
                return r;
            }
            match r {
                Ok(vals) => return Ok(vals),
                // unwind toward `entry_depth`. A protecting pcall/xpcall
                // continuation caught along the way turns the error into
                // `false, msg` and the loop resumes running its caller; an
                // uncaught error propagates out.
                Err(e) => match self.unwind(e.0, entry_depth) {
                    Unwound::Caught => continue,
                    Unwound::CaughtReturn(vals) => return Ok(vals),
                    Unwound::Propagated(err) => return Err(err),
                },
            }
        }
    }

    /// Unwind the call stack from the error point toward `entry_depth`, running
    /// `__close` handlers on each Lua frame. Stops at the first pcall/xpcall
    /// continuation frame at/above `entry_depth` (the error is *caught*: its
    /// slot receives `false, msg`); if none is reached, the error propagates.
    fn unwind(&mut self, mut err: Value, entry_depth: usize) -> Unwound {
        // The protected call runs in-place among the caller frames' registers,
        // so truncating the failed frames here cuts into caller windows below
        // the catcher. Snapshot the live length: at the error point the stack
        // already spans every surviving frame's window, so restoring it after a
        // catch reinstates them all (the reclaimed slots above are dead temps).
        // PUC handles overflow recovery via a separate EXTRA_STACK reserve;
        // we instead clamp the restore to the catcher's caller window when the
        // error point was at the stack limit (cause: the next `call_value_impl`
        // picks `func_slot = stack.len()` which would otherwise re-overflow).
        let saved_len = self.stack.len();
        err = self.raise_to_handler(err);
        // An error that no protected call inside the running coroutine will
        // catch kills it without unwinding: PUC's `lua_resume` leaves the
        // dead thread's stack as it was, so its pending to-be-closed
        // variables run only when it is closed (`coroutine.close`, or
        // `coroutine.wrap` closing it before re-raising). Scoped to the
        // coroutine's own run (`entry_depth == 1`); a run nested under a
        // native unwinds as before.
        if entry_depth == 1
            && self.version >= LuaVersion::Lua54
            && self
                .current
                .is_some_and(|c| c.status == crate::runtime::CoroStatus::Running)
            && !self.frames.iter().any(|f| {
                matches!(
                    f,
                    CallFrame::Cont(NativeCont {
                        kind: ContKind::Pcall | ContKind::Xpcall { .. } | ContKind::Close(_),
                        ..
                    })
                )
            })
        {
            while self.frames.len() >= entry_depth {
                frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            }
            return Unwound::Propagated(LuaError(err));
        }
        while self.frames.len() >= entry_depth {
            match *self.frames.last().expect("frame") {
                // a yieldable-metamethod continuation does not catch: discard the
                // abandoned instruction and keep unwinding (PUC drops the partial
                // op on error).
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Meta(mc),
                    func_slot,
                    ..
                }) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.stack.truncate(func_slot as usize);
                    self.top = mc.saved_top.min(func_slot);
                    self.tbc.retain(|&s| s < func_slot);
                }
                // a __pairs continuation does not catch either: an error inside
                // the metamethod propagates past `pairs`.
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Pairs,
                    func_slot,
                    ..
                }) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.stack.truncate(func_slot as usize);
                    self.top = func_slot;
                    self.tbc.retain(|&s| s < func_slot);
                }
                // a __close continuation does not catch: drop the half-run
                // handler's window, then continue the close yieldably with
                // the new error threaded as `pending`. Preserve `cc.after`
                // verbatim — `Return`/`Block` originating from an aborting
                // OP_Return/OP_Close will be short-circuited by
                // `finish_close_after` (pending propagates as Err); a
                // `ResumeUnwind` originated by our own Lua-frame handler
                // must keep its deferred frame-pop semantics so that frame
                // is not orphaned. If a fresh handler yields, `drive_close`
                // pushes another `Cont::Close` and we return `Caught` so
                // `exec_with` re-enters the run loop.
                CallFrame::Cont(NativeCont {
                    kind: ContKind::Close(cc),
                    func_slot,
                    ..
                }) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.stack.truncate(func_slot as usize);
                    self.top = func_slot;
                    self.tbc.retain(|&s| s < func_slot);
                    match self.drive_close(cc.from, Some(err), cc.after, entry_depth) {
                        Ok(Some(_)) => {
                            unreachable!(
                                "Block / Return / ResumeUnwind never return host values mid-unwind"
                            )
                        }
                        Ok(None) => return Unwound::Caught,
                        Err(e) => {
                            // the drained close re-raises `err`; only an
                            // error a handler raised is new
                            if !e.0.raw_eq(err) {
                                err = self.raise_to_handler(e.0);
                            }
                            continue;
                        }
                    }
                }
                CallFrame::Cont(nc) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    self.pcall_depth -= 1;
                    let result = match nc.kind {
                        ContKind::Pcall => {
                            self.msgh_applied = None;
                            err
                        }
                        // the handler ran where the error was raised (see
                        // `raise_to_handler`); one raised past the handler's
                        // reach (by the unwind itself) meets it here
                        ContKind::Xpcall { handler } => {
                            if self.msgh_applied.take().is_some_and(|v| v.raw_eq(err)) {
                                err
                            } else {
                                self.call_msgh(handler, err)
                            }
                        }
                        ContKind::Meta(_) | ContKind::Pairs | ContKind::Close(_) => {
                            unreachable!("Meta/Pairs/Close cont handled above")
                        }
                    };
                    // PUC 5.5 `luaG_errormsg` substitutes "<no error object>"
                    // for nil AFTER the message handler ran (ldebug.c:849) —
                    // so it applies to the pcall-caught object and to an
                    // xpcall HANDLER'S return value, while the handler itself
                    // (and a top-level propagation into the host, whose
                    // `error_display` plays msghandler) still sees the raw
                    // nil. 5.4- keep nil everywhere (errors.lua :49 asserts
                    // `doit("error()") == nil`).
                    let result = if matches!(result, Value::Nil)
                        && self.version >= crate::version::LuaVersion::Lua55
                    {
                        Value::Str(self.heap.intern(b"<no error object>"))
                    } else {
                        result
                    };
                    // the error has been caught (pcall/xpcall): the captured
                    // traceback was for that error and is no longer in flight.
                    self.error_traceback = None;
                    let fs = nc.func_slot as usize;
                    if self.stack.len() < fs + 2 {
                        self.stack.resize(fs + 2, Value::Nil);
                    }
                    self.stack[fs] = Value::Bool(false);
                    self.stack[fs + 1] = result;
                    self.top = nc.func_slot + 2;
                    self.tbc.retain(|&s| s < nc.func_slot);
                    if self.frames.len() < entry_depth {
                        return Unwound::CaughtReturn(self.take_results(nc.func_slot));
                    }
                    self.finish_results(nc.func_slot, 2, nc.nresults);
                    // reinstate the caller windows the unwind truncated into,
                    // clamped to the catcher's caller window + a `MIN_STACK`
                    // reserve. The clamp is a no-op for normal pcall catches
                    // (saved_len lies within the caller's max_stack window),
                    // and prevents the stack from staying near `MAX_LUA_STACK`
                    // after an overflow-recovery catch — which would make the
                    // next `call_value_impl` (e.g. a `__close` in the catcher's
                    // errorh, locals.lua:659) pick `func_slot = stack.len()`
                    // above the limit and re-overflow.
                    // Restore the caller's full register window: opcodes
                    // index it directly. The cap covers caller's base +
                    // `max_stack` + a small reserve. We always resize to
                    // exactly this window — previously this clamped
                    // `saved_len` from above to prevent staying near
                    // `MAX_LUA_STACK` after an overflow-recovery catch, and
                    // a yieldable-unwind re-entry adds the dual case where
                    // `saved_len` is *below* the window (a prior
                    // `ResumeUnwind` truncated). Using the window directly
                    // covers both.
                    let restore = self
                        .frames
                        .iter()
                        .rev()
                        .find_map(CallFrame::lua)
                        .map(|c| (c.base + c.closure.proto.max_stack as u32) as usize + 256)
                        .unwrap_or(saved_len);
                    if self.stack.len() < restore {
                        self.stack.resize(restore, Value::Nil);
                    } else if self.stack.len() > restore {
                        self.stack.truncate(restore);
                    }
                    // Clear slots vacated by the popped
                    // frames the unwind walked over. finish_results
                    // above clears `[nc.func_slot + nresults ..
                    // nc.func_slot + 2)`, which only covers the
                    // pcall's own result region — the unwind-popped
                    // frames' locals in `[nc.func_slot + 2 .. restore)`
                    // are still in place with whatever Gc-bearing
                    // Values they last held. Without this clear, a
                    // later GC marks the stale pointers (same hazard as
                    // the Op::Return finish_results path). PUC's `luaD_pcall` similarly truncates
                    // L->top to the catcher's level — luna's
                    // truncate above resizes the Vec but doesn't
                    // touch slots [func_slot+2..restore) that were
                    // already present.
                    let clear_lo = (nc.func_slot as usize + 2).min(self.stack.len());
                    let clear_hi = restore.min(self.stack.len());
                    if clear_lo < clear_hi {
                        for slot in &mut self.stack[clear_lo..clear_hi] {
                            *slot = Value::Nil;
                        }
                    }
                    return Unwound::Caught;
                }
                CallFrame::Lua(f) => {
                    // Yieldable error-unwind close, PUC luaG_errormsg shape:
                    // (1) pop the Lua frame immediately so each `__close`
                    // handler runs at the C boundary above — `debug.getinfo`
                    // sees the next outer Lua frame's call site (typically
                    // `pcall`), not this aborting function (locals.lua:480).
                    // (2) drive the close yieldably with
                    // `AfterClose::ResumeUnwind { func_slot, err }`; on drain
                    // it truncates to `func_slot` and re-raises (letting a
                    // handler-raised error win over `err`). If a handler
                    // yields, `drive_close` pushes `Cont::Close` and we
                    // return `Caught` so `exec_with` re-enters the run loop;
                    // a synchronous drain returns Err exactly as the old
                    // path did.
                    frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                    let after = AfterClose::ResumeUnwind {
                        func_slot: f.func_slot,
                        err,
                    };
                    match self.begin_close(f.base, Some(err), after, entry_depth) {
                        Ok(Some(_)) => {
                            unreachable!("ResumeUnwind never returns host values")
                        }
                        Ok(None) => return Unwound::Caught,
                        Err(e) => {
                            // the drained close re-raises `err`; only an
                            // error a handler raised is new
                            if !e.0.raw_eq(err) {
                                err = self.raise_to_handler(e.0);
                            }
                            continue;
                        }
                    }
                }
            }
        }
        Unwound::Propagated(LuaError(err))
    }

    /// The loop head's slow path, taken while [`Vm::trap`] is set: tick the
    /// instruction budget, enforce the memory cap, then clear `trap` unless
    /// one of them, an armed hook or a continuation frame on top still needs
    /// the next pass through the loop head.
    #[inline]
    fn trap_step(&mut self) -> Result<(), LuaError> {
        if let Some(b) = self.instr_budget.as_mut() {
            *b -= 1;
            if *b <= 0 {
                return Err(self.instr_budget_exhausted());
            }
        }
        if let Some(cap) = self.heap.mem_cap
            && self.heap.bytes() > cap
        {
            self.mem_cap_exceeded(cap)?;
        }
        self.trap = self.instr_budget.is_some()
            || self.heap.mem_cap.is_some()
            || self.hook_armed()
            || matches!(self.frames.last(), Some(CallFrame::Cont(_)));
        Ok(())
    }

    /// A count or line hook fires on the next instruction.
    #[inline]
    fn hook_armed(&self) -> bool {
        !self.in_hook && (self.hook.func.is_some() || self.hook.rust_func.is_some())
    }

    /// Count and line hooks (PUC `traceexec`), fired before the instruction
    /// at `pc` of `cl` runs; `oldpc` is where the frame's line hook last looked.
    #[inline(never)]
    fn exec_hooks(&mut self, cl: Gc<LuaClosure>, pc: u32, oldpc: u32) -> Result<(), LuaError> {
        let lines = &cl.proto.lines;
        let cur_line = if lines.is_empty() {
            None
        } else {
            Some(lines[(pc as usize).min(lines.len() - 1)] as i64)
        };
        // count hook: fire every `count_base` instructions
        if self.hook.count {
            self.hook.count_left -= 1;
            if self.hook.count_left <= 0 {
                self.hook.count_left = self.hook.count_base;
                // hooked function is the running Lua frame: its frame
                // is on the stack, so no synthetic C level is needed.
                self.run_hook(b"count", cur_line, false)?;
            }
        }
        // line hook: fire on a fresh frame, a backward jump (loop), or a
        // change of source line.
        if self.hook.line {
            if lines.is_empty() {
                // PUC: a stripped chunk has no line info, so
                // `getfuncline` returns -1. The line hook still fires
                // on the first instruction of the new frame (where
                // `npci <= oldpc` holds at oldpc=0), with the line
                // pushed as `nil` instead of an integer (db.lua :1030
                // "hook called without debug info for 1st instruction").
                if oldpc == u32::MAX {
                    self.run_hook(b"line", None, false)?;
                    self.top_frame_mut().hook_oldpc = pc;
                }
            } else {
                let newline = lines[(pc as usize).min(lines.len() - 1)];
                // PUC `traceexec`: fire on frame entry (`oldpc == MAX`),
                // on a backward jump (`pc < oldpc` — strict; an equal pc
                // would re-fire the install-site after `oldpc = pc`),
                // or when the source line changes.
                let fire = oldpc == u32::MAX
                    || pc < oldpc
                    || newline != lines[(oldpc as usize).min(lines.len() - 1)];
                if fire {
                    self.run_hook(b"line", Some(newline as i64), false)?;
                }
                self.top_frame_mut().hook_oldpc = pc;
            }
        }
        Ok(())
    }

    /// A continuation frame is on top: the call it protected has delivered
    /// its results (or a `__close` handler / yieldable metamethod finished).
    /// `Some` hands results out of this activation.
    #[inline(never)]
    fn finish_cont(
        &mut self,
        nc: NativeCont,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        // a yieldable metamethod returned: complete the interrupted
        // instruction (PUC luaV_finishOp) and resume the running frame.
        if let ContKind::Meta(mc) = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            let result = if self.top > nc.func_slot {
                self.stack[nc.func_slot as usize]
            } else {
                Value::Nil
            };
            self.stack.truncate(nc.func_slot as usize);
            self.top = mc.saved_top;
            self.finish_meta(mc.action, result)?;
            return Ok(None);
        }
        // a __close handler returned successfully: discard its
        // results, restore `top` to the slot the handler was called
        // at (the surrounding frame's register window above this slot
        // must stay alloc'd — never truncate the underlying stack),
        // then continue the close chain (next slot, or fire
        // AfterClose). When the close ends an entry activation,
        // drive_close hands the results up to exec_with directly.
        if let ContKind::Close(cc) = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            self.top = nc.func_slot;
            if let Some(vals) = self.drive_close(cc.from, cc.pending, cc.after, entry_depth)? {
                return Ok(Some(vals));
            }
            return Ok(None);
        }
        // __pairs returned: normalize its results to exactly the
        // dialect's count (iterator, state, control, and on 5.5 the
        // closing value) at pairs's slot, where the metamethod was
        // called, and hand them to pairs's caller.
        if let ContKind::Pairs = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            let total = crate::vm::builtins::pairs_mm_results(self) as u32;
            let need = (nc.func_slot + total) as usize;
            if self.stack.len() < need {
                self.stack.resize(need, Value::Nil);
            }
            // the metamethod ran one slot above pairs's own
            let first = nc.func_slot + 1;
            let n = (self.top - first).min(total);
            for i in 0..n {
                self.stack[(nc.func_slot + i) as usize] = self.stack[(first + i) as usize];
            }
            for s in (nc.func_slot + n)..(nc.func_slot + total) {
                self.stack[s as usize] = Value::Nil;
            }
            self.top = nc.func_slot + total;
            if self.frames.len() < entry_depth {
                return Ok(Some(self.take_results(nc.func_slot)));
            }
            self.finish_results(nc.func_slot, total, nc.nresults);
            return Ok(None);
        }
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        self.pcall_depth -= 1;
        // f's results sit at nc.func_slot+1.. (f was called one slot
        // above the continuation), so writing `true` at the slot makes
        // `true, results…` already contiguous.
        let nret = self.top - (nc.func_slot + 1);
        self.stack[nc.func_slot as usize] = Value::Bool(true);
        let total = 1 + nret;
        self.top = nc.func_slot + total;
        if self.frames.len() < entry_depth {
            return Ok(Some(self.take_results(nc.func_slot)));
        }
        self.finish_results(nc.func_slot, total, nc.nresults);
        Ok(None)
    }

    fn run(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        // the host may have set a budget, a cap or a hook since the last run
        self.trap = true;
        let pre53 = self.version() <= LuaVersion::Lua53;
        loop {
            if self.trap {
                self.trap_step()?;
            }
            // A continuation frame on top means the call it protected just
            // delivered its results. Pushing a continuation, or popping down
            // to one, sets `trap` (`frames_push_sync` / `frames_pop_sync`), so
            // the loop head looks for one only under `trap`.
            if self.trap
                && let Some(&CallFrame::Cont(nc)) = self.frames.last()
            {
                if let Some(vals) = self.finish_cont(nc, entry_depth)? {
                    return Ok(vals);
                }
                continue;
            }
            debug_assert!(
                matches!(self.frames.last(), Some(CallFrame::Lua(_))),
                "a continuation frame on top with `trap` clear"
            );
            // GC runs only at the allocation safe points below (PUC's
            // `luaC_checkGC` sites), each with a precise `gc_top`; the loop head
            // no longer collects, so a stale full-window `gc_top` cannot leak in.
            //
            // Hot-path frame fetch: the Cont arm above continues the loop,
            // so reaching here means `frame_peek` is the Lua frame. Reuse it
            // rather than re-fetching `self.frames.last()`.
            // SAFETY: the running thread always has a frame here, and it is a
            // Lua frame: a continuation on top was consumed above
            let f = match unsafe { self.frames.last().unwrap_unchecked() } {
                CallFrame::Lua(f) => f,
                // SAFETY: see above
                CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
            };
            let cl = f.closure;
            let base = f.base;
            let func_slot = f.func_slot;
            let n_varargs = f.n_varargs;
            let pc = f.pc;
            let oldpc = f.hook_oldpc;

            // SAFETY: `pc` is bounded by the compiler against `proto.code.len()`
            // — every branch / call op only sets `pc` to a valid index, and
            // function entry initialises pc=0 with a non-empty body. PUC's
            // `vmfetch` uses the equivalent unchecked load.
            let inst = unsafe { *cl.proto.code.get_unchecked(pc as usize) };

            // Trace recording append + close detection.
            // Gated on `trace_jit_enabled` + `active_trace.is_some()`
            // so default dispatch keeps a single not-taken branch.
            //
            // - At the head PC with a non-empty record, the trace has
            //   looped back to its start: mark `closed = true` and
            //   take the record for compile + cache.
            // - Otherwise, capture the op. If the record overflows
            //   MAX_TRACE_LEN, abort by dropping it.
            // read once: with the trace JIT off this is the only JIT test an
            // instruction makes
            let trace_on = self.jit.trace_enabled;
            if trace_on && self.jit.active_trace.is_some() {
                self.trace_record_step(cl, pc, inst, base);
            }

            // Trace JIT dispatcher.
            //
            // When the dispatch loop is about to execute the op at
            // `pc` and there's a `numeric_only` CompiledTrace cached
            // for that `head_pc`, marshal the live regs into an
            // i64 buffer, jump into the trace, and resume the
            // interpreter at the returned continuation PC.
            //
            // Skipped when `trace_jit_enabled` is false or the Proto
            // holds no trace the lookup could admit
            // (`has_dispatchable_trace`); otherwise the lookup is a
            // borrow + scan over `cl.proto.traces`.
            //
            // Marshalling contract — only Int slots survive the
            // round-trip cleanly (the reg_state ABI is `*mut i64`
            // with no tag info). Any non-Int slot in the affected
            // window forces a skip; interp takes over for one op
            // and the back-edge brings us back to try again next
            // pass (slots that were Nil/Float at one moment can
            // settle to Int by the time the next back-edge fires).
            //
            // A trace that comes back with `vm.jit.pending_err`
            // parked is treated as a deopt: clear the err, leave
            // the stack as the trace wrote it, and let the
            // interpreter run from the same `pc`. The trace itself
            // is left cached — a future entry might find no
            // metatable in the way and succeed.
            // Single Rc<CompiledTrace> clone instead of per-field Rc
            // clones: proto.traces is Vec<Rc<CompiledTrace>>; the
            // dispatcher clones ONE Rc and reads fields via auto-deref.
            // One-shot consume of the
            // `suppress_downrec_admit_once` flag. Set by the
            // downrec post-invoke arm below when it force-deopts the
            // trace (caller-pc guard miss OR cycle-budget exhausted)
            // so the NEXT interpreter loop iteration skips the
            // downrec admit, lets interp run the op at `head_pc`,
            // advances `pc` past `head_pc`, and breaks the otherwise-
            // infinite admit loop. Reading + clearing here means a
            // single dispatch tick consumes the suppression — the
            // following tick re-admits naturally (with the budget
            // also reset by the deopt site).
            // The one-shot suppression only matters where a downrec trace
            // could be admitted, which needs the proto's flag.
            let admit = trace_on && cl.proto.has_dispatchable_trace.get();
            let downrec_admit_blocked =
                admit && std::mem::take(&mut self.jit.suppress_downrec_admit_once);
            if admit && self.trace_dispatch(cl, pc, base, downrec_admit_blocked) {
                continue;
            }

            // PUC `vmfetch` increments savedpc BEFORE firing traceexec, so
            // hook code that consults `currentpc = savedpc - 1` lands on the
            // instruction now executing. luna mirrors that by advancing
            // `f.pc` to `pc + 1` before the hook block — local_at /
            // getinfo / line attribution all read f.pc, and the existing
            // `pc - 1` convention in those helpers then yields the current
            // instruction's pc (db.lua :696: local `A` visible at the
            // chunk's return line once OP_CLOSURE has advanced pc).
            //
            // Inline `top_frame_mut` for the hot path: top is guaranteed Lua
            // (cont frames drained above) so the and_then/Option layers are
            // dead weight.
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            let mut fpc: *mut u32 = match unsafe { self.frames.last_mut().unwrap_unchecked() } {
                CallFrame::Lua(fmut) => {
                    fmut.pc = pc + 1;
                    &mut fmut.pc
                }
                _ => unreachable!("Cont frame at pc bump"),
            };

            // count + line hooks (PUC traceexec): before executing the
            // instruction. Skipped while the hook itself runs. `trap` is set
            // whenever a hook is armed, so the common case tests one byte.
            if self.trap && self.hook_armed() {
                self.exec_hooks(cl, pc, oldpc)?;
                // a hook runs Lua code, which can move `self.frames`
                // SAFETY: as above
                fpc = match unsafe { self.frames.last_mut().unwrap_unchecked() } {
                    CallFrame::Lua(fmut) => &mut fmut.pc,
                    _ => unreachable!("Cont frame after a hook"),
                };
            }

            let fx = fast::Fast {
                cl,
                base,
                func_slot,
                n_varargs,
                fpc,
                trace_on,
                pre53,
                entry_depth,
            };
            let inst = match self.run_fast(fx, inst, pc + 1)? {
                fast::FastExit::Reload => continue,
                fast::FastExit::Slow(inst) => inst,
            };
            // the fast loop may have called or returned into another frame
            let &Frame {
                closure: cl,
                base,
                func_slot,
                n_varargs,
                ..
            } = self.top_frame();
            match inst.op() {
                Op::LoadKx => {
                    let extra = cl.proto.code[self.pc_of_top() as usize];
                    self.bump_pc();
                    let v = cl.proto.consts[extra.ax() as usize];
                    self.set_r(base, inst.a(), v);
                }
                Op::NewTable => {
                    let t = self.heap.new_table();
                    self.set_r(base, inst.a(), Value::Table(t));
                    self.maybe_collect_garbage(base + inst.a() + 1);
                }
                Op::SetList => {
                    let a = inst.a();
                    let abs_a = base + a;
                    // only `debug.setlocal` or crafted bytecode can put a
                    // non-table here; PUC crashes, luna raises
                    let t = match self.r(base, a) {
                        Value::Table(t) => t,
                        v => return Err(self.type_err("index", v)),
                    };
                    let n = if inst.b() == 0 {
                        self.top - (abs_a + 1)
                    } else {
                        inst.b()
                    };
                    let offset = if inst.k() {
                        let extra = cl.proto.code[self.pc_of_top() as usize];
                        self.bump_pc();
                        extra.ax() as i64
                    } else {
                        inst.c() as i64
                    };
                    for i in 1..=n {
                        let v = self.r(base, a + i);
                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                        if let Err(TableError::Overflow) =
                            unsafe { t.as_mut() }.set_int(&mut self.heap, offset + i as i64, v)
                        {
                            return Err(self.rt_err("table overflow"));
                        }
                    }
                    // one barrier_back covers every store this op did — PUC's
                    // `luaC_barrierback_` once-per-table optimisation
                    self.heap
                        .barrier_back(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
                    // the element temps above the table are now consumed
                    self.maybe_collect_garbage(base + a + 1);
                }
                Op::Pow => {
                    let (l, r) = (self.r(base, inst.b()), self.r(base, inst.c()));
                    self.arith_slow(inst.a(), base, ArithOp::Pow, l, r, false)?
                }
                Op::Concat => {
                    // right-associative fold over operands at base+a .. base+a+n,
                    // in place on the stack so a yielding __concat can suspend.
                    let a = inst.a();
                    let n = inst.b();
                    self.top = base + a + n;
                    self.concat_run(base + a)?;
                }
                Op::Close => {
                    // Yieldable: drive __close handlers through the
                    // interpreter loop so a coroutine.yield() inside a
                    // handler suspends cleanly (locals.lua block-end yield).
                    // `drive_close` parks the handler call at `self.top`, so
                    // raise `top` past this frame's full register window
                    // first — a goto out of a nested for-loop can fire
                    // OP_Close while `self.top` still sits at the inner
                    // body's working top, which would let `push_frame`'s
                    // wipe clobber the outer tbc slot before it could be
                    // closed (locals.lua:1219 nested-for goto regression).
                    self.top = self.top.max(base + cl.proto.max_stack as u32);
                    let _ =
                        self.begin_close(base + inst.a(), None, AfterClose::Block, entry_depth)?;
                }
                Op::Tbc => {
                    self.register_tbc(base + inst.a())?;
                }
                Op::TailCall => {
                    let fr = *self.top_frame();
                    let abs = base + inst.a();
                    let mut nargs = if inst.b() == 0 {
                        self.top - (abs + 1)
                    } else {
                        inst.b() - 1
                    };
                    // A tail call pops this frame before begin_call, so a
                    // non-callable target would lose its name/position. Report
                    // it now (PUC reads funcname from the still-current ci),
                    // while the frame is intact, for "(field 'x')"-style info.
                    let mut func = self.stack[abs as usize];
                    if !matches!(func, Value::Closure(_) | Value::Native(_))
                        && self.get_mm(func, Mm::Call).is_nil()
                    {
                        return Err(self.call_err(func));
                    }
                    // PUC `luaD_pretailcall` resolves a chain of `__call`
                    // metamethods *in place* before deciding whether to
                    // collapse this frame. Without that, each __call hop
                    // would push a fresh Lua frame and a 10000-deep
                    // tail-recursion through a 100-deep __call chain
                    // (5.4 calls.lua :172) blows up. Mirror the PUC loop:
                    // shift args right, install the handler at `abs`, retry.
                    // Chain depth limit matches the call-site `begin_call`
                    // version cap (5.5 calls.lua :223 — 15 max, then "too
                    // long"; 16th wrap fails the call). An infinite
                    // self-referential `__call` would otherwise spin.
                    let chain_cap = if self.version >= LuaVersion::Lua55 {
                        15
                    } else {
                        MAX_CCMT
                    };
                    let mut chain = 0u32;
                    while !matches!(func, Value::Closure(_) | Value::Native(_)) {
                        let mm = self.get_mm(func, Mm::Call);
                        if mm.is_nil() || self.call_mm_unusable(mm) {
                            return Err(self.call_err(func));
                        }
                        chain += 1;
                        if chain > chain_cap {
                            return Err(self.rt_err("'__call' chain too long"));
                        }
                        let end = (abs + 1 + nargs) as usize;
                        if self.stack.len() < end + 1 {
                            self.stack.resize(end + 1, Value::Nil);
                        }
                        for i in (0..=nargs).rev() {
                            self.stack[(abs + 1 + i) as usize] = self.stack[(abs + i) as usize];
                        }
                        self.stack[abs as usize] = mm;
                        nargs += 1;
                        self.top = abs + 1 + nargs;
                        func = mm;
                    }
                    // PUC's tail-call collapse is Lua→Lua only. A tail call to
                    // a C function runs the C function under the *current* Lua
                    // activation (no frame fold — a C frame has nothing to
                    // collapse into); after the C function returns, the
                    // calling Lua function returns those results normally.
                    // Mirror that: keep our Lua frame on the stack, call the
                    // target through `begin_call(abs, …)` as a regular call,
                    // and let the fallback `Op::Return` that the compiler
                    // emits right after `Op::TailCall` forward the results.
                    // 5.1 closure.lua :177's `return getfenv()` from inside
                    // foo needs level 1 to resolve to foo, not to the
                    // thread's globals fallback that happens when no Lua
                    // frame is on the stack.
                    let lua_target = matches!(func, Value::Closure(_));
                    if lua_target {
                        self.close_slots(fr.base, None)?;
                        for i in 0..=nargs {
                            self.stack[(fr.func_slot + i) as usize] =
                                self.stack[(abs + i) as usize];
                        }
                        // Clear the slot range that's now
                        // stranded by the tail-call collapse. The args
                        // were copied to `[fr.func_slot..fr.func_slot+
                        // nargs+1)`; the source slots `[abs..abs+
                        // nargs+1)` still hold the same `Value::Closure
                        // / Value::Str / ...` entries, but they're past
                        // the new call's window. Without this clear, a
                        // later GC with wider gc_top would mark stale
                        // pointers there (same hazard the
                        // finish_results slot-clear closes for the
                        // Op::Return path).
                        let new_top_lower_bound = fr.func_slot + nargs + 1;
                        let prev_top = (self.top as usize).min(self.stack.len());
                        if (new_top_lower_bound as usize) < prev_top {
                            for slot in &mut self.stack[new_top_lower_bound as usize..prev_top] {
                                *slot = Value::Nil;
                            }
                        }
                        // PUC `CIST_TAIL`: the new Lua activation inherits
                        // the popped frame's tailcalls count plus one for
                        // this collapse. 5.1 db.lua :372 hammers 30000
                        // recursive tail calls and expects to see the
                        // synthetic tail level for every one of them.
                        self.pending_tailcalls = fr.tailcalls.saturating_add(1);
                        self.pending_ccmt = fr.ccmt;
                        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
                        if !self.begin_call(fr.func_slot, Some(nargs), fr.nresults, false)?
                            && self.frames.len() < entry_depth
                        {
                            // a native completed what was this function's result
                            return Ok(self.take_results(fr.func_slot));
                        }
                    } else {
                        // Native (or __call-bearing) target: regular call. The
                        // results land at `abs..self.top` and the next op (the
                        // fallback `Op::Return`) forwards them. `wanted = -1`
                        // because the caller will multret them through Return.
                        // PUC's precallC gives the C call this tail call's own
                        // `__call` count (5.5 extraargs); the chain was already
                        // resolved above, so hand it over.
                        self.pending_ccmt = chain as u8;
                        self.begin_call(abs, Some(nargs), -1, false)?;
                    }
                }
                Op::Return | Op::Return0 | Op::Return1 => {
                    let (abs_a, nret) = match inst.op() {
                        Op::Return0 => (base, 0),
                        Op::Return1 => (base + inst.a(), 1),
                        _ => {
                            let abs_a = base + inst.a();
                            let nret = if inst.b() == 0 {
                                self.top - abs_a
                            } else {
                                inst.b() - 1
                            };
                            (abs_a, nret)
                        }
                    };
                    // close before moving results: __close handlers run above
                    // the stack top, so the result region [abs_a..abs_a+nret)
                    // stays intact across any yields the close performs.
                    // Fixed-count returns may leave `self.top` below the last
                    // result slot (the compiler does not always re-bump it);
                    // raise it past the result region so `drive_close` parks
                    // the handler call *above* — landing at `self.top` would
                    // otherwise clobber a result with the handler closure.
                    self.top = self.top.max(abs_a + nret);
                    if matches!(inst.op(), Op::Return0 | Op::Return1)
                        && self.return_to_lua(base, abs_a, nret, entry_depth)
                    {
                        // done: the caller's frame is on top
                    } else if let Some(vals) = self.begin_close(
                        base,
                        None,
                        AfterClose::Return {
                            abs_a,
                            nret,
                            from_native: false,
                        },
                        entry_depth,
                    )? {
                        return Ok(vals);
                    }
                }
                Op::ForPrep => self.for_prep(inst, base)?,
                Op::TForPrep => {
                    // the 4th control slot is the iterator's closing value
                    self.register_tbc(base + inst.a() + 3)?;
                    self.add_pc(inst.bx() as i32);
                }
                Op::TForCall => {
                    let abs = base + inst.a();
                    let need = (abs + 7) as usize;
                    if self.stack.len() < need {
                        self.stack.resize(need, Value::Nil);
                    }
                    self.stack[(abs + 4) as usize] = self.stack[abs as usize];
                    self.stack[(abs + 5) as usize] = self.stack[(abs + 1) as usize];
                    self.stack[(abs + 6) as usize] = self.stack[(abs + 2) as usize];
                    let nvars = inst.c() as i32;
                    self.begin_call(abs + 4, Some(2), nvars, false)?;
                }
                Op::Closure => {
                    let proto = cl.proto.protos[inst.bx() as usize];
                    let n_ups = proto.upvals.len();
                    // Build upvals on the stack for small
                    // closures, skipping the per-call Vec/Box alloc
                    // that closure_alloc's 10k iters pay. INLINE_UPVALS_N
                    // = 2 covers most Lua source (1 captured local, or
                    // _ENV + a single capture). Beyond that, fall back
                    // to a heap Vec.
                    use crate::runtime::function::INLINE_UPVALS_N;
                    let mut stack_buf: [std::mem::MaybeUninit<
                        Gc<crate::runtime::function::Upvalue>,
                    >; INLINE_UPVALS_N] = [std::mem::MaybeUninit::uninit(); INLINE_UPVALS_N];
                    let mut heap_buf: Vec<Gc<crate::runtime::function::Upvalue>> = Vec::new();
                    let use_inline = n_ups <= INLINE_UPVALS_N;
                    if !use_inline {
                        heap_buf.reserve_exact(n_ups);
                    }
                    for (i, d) in proto.upvals.iter().enumerate() {
                        let uv = if d.in_stack {
                            self.find_or_create_upval(base + d.index as u32)
                        } else {
                            cl.upvals()[d.index as usize]
                        };
                        if use_inline {
                            stack_buf[i] = std::mem::MaybeUninit::new(uv);
                        } else {
                            heap_buf.push(uv);
                        }
                    }
                    // Tiny shim around the two paths so the 5.1 _ENV
                    // clone + cache check below see one uniform
                    // `&mut [Gc<Upvalue>]`. The stack_buf slice points
                    // into the local frame (still valid through the
                    // rest of this Op::Closure handler).
                    let ups: &mut [Gc<crate::runtime::function::Upvalue>] = if use_inline {
                        // SAFETY: the first n_ups slots of stack_buf
                        // were initialised above; we hand out a slice
                        // covering exactly them.
                        unsafe {
                            std::slice::from_raw_parts_mut(
                                stack_buf.as_mut_ptr()
                                    as *mut Gc<crate::runtime::function::Upvalue>,
                                n_ups,
                            )
                        }
                    } else {
                        &mut heap_buf[..]
                    };
                    // PUC 5.1 had per-function environments: every Lua
                    // function carried its own `env` slot, snapshotted from
                    // the creating function's env at closure time, so a
                    // `setfenv` on one closure never bled into a sibling.
                    // luna models that by giving the 5.1 closure a *fresh*
                    // closed upvalue for whichever cell holds `_ENV`, seeded
                    // from the parent's current env value. Only that cell is
                    // cloned — every other upvalue keeps its open/shared
                    // identity (so e.g. `local function range(...) ...
                    // range(...) ... end` still sees its self-reference). 5.2+
                    // keeps the shared-upval model (and the proto cache that
                    // depends on it).
                    let v51 = self.version() <= LuaVersion::Lua51;
                    if v51 && proto.env_upval_idx != u8::MAX {
                        let i = proto.env_upval_idx as usize;
                        let cur = match ups[i].state() {
                            UpvalState::Open { slot, thread } => self.read_slot(slot, thread),
                            UpvalState::Closed(v) => v,
                        };
                        ups[i] = self.heap.new_upvalue(UpvalState::Closed(cur));
                    }
                    let ups_slice: &[Gc<crate::runtime::function::Upvalue>] = ups;
                    // PUC 5.2+ `getcached`: a Proto remembers its last LClosure
                    // and reuses it when every fresh-upvalue binding still
                    // points to the same Upvalue object as the cached one.
                    // That keeps `function() return outer end` repeated in a
                    // loop comparing equal across iterations (the captured
                    // outer is a shared open upvalue), while `function()
                    // return loop_var end` gets a fresh closure each round
                    // because the loop var is re-created per iteration. PUC
                    // 5.1 predated the cache, and the per-closure `_ENV`
                    // clone above would defeat it anyway, so skip it.
                    let nc = if v51 {
                        self.heap.new_closure_inline(proto, ups_slice)
                    } else {
                        let cached = proto.cache.get().filter(|c| {
                            c.upvals().len() == ups_slice.len()
                                && c.upvals()
                                    .iter()
                                    .zip(ups_slice.iter())
                                    .all(|(a, b)| std::ptr::eq(a.as_ptr(), b.as_ptr()))
                        });
                        match cached {
                            Some(c) => c,
                            None => {
                                let n = self.heap.new_closure_inline(proto, ups_slice);
                                proto.cache.set(Some(n));
                                n
                            }
                        }
                    };
                    self.set_r(base, inst.a(), Value::Closure(nc));
                    self.maybe_collect_garbage(base + inst.a() + 1);
                }
                Op::Vararg => {
                    let abs_a = base + inst.a();
                    let wanted = inst.c() as i32 - 1;
                    // A materialized named vararg lives in func_slot (its writes
                    // must be visible to `...`); otherwise spread the extra args
                    // straight off the stack at func_slot+1 .. +n_varargs.
                    let vt = match self.stack[func_slot as usize] {
                        Value::Table(t) => Some(t),
                        _ => None,
                    };
                    let n = match vt {
                        Some(t) => {
                            let n_key = Value::Str(self.heap.intern(b"n"));
                            // PUC getnumargs: a named vararg `t.n` set out of the
                            // integer range [0, INT_MAX/2] is rejected here
                            match t.get(n_key) {
                                Value::Int(n) if (n as u64) <= (i32::MAX as u64 / 2) => n as u32,
                                _ => return Err(self.rt_err("vararg table has no proper 'n'")),
                            }
                        }
                        None => n_varargs,
                    };
                    let count = if wanted < 0 { n } else { wanted as u32 };
                    // a named vararg's `n` can be set to anything up to
                    // INT_MAX/2; PUC's `luaD_checkstack` refuses what the
                    // stack cannot hold
                    if abs_a + count > MAX_LUA_STACK {
                        return Err(self.rt_err("stack overflow"));
                    }
                    let need = (abs_a + count) as usize;
                    if self.stack.len() < need {
                        self.stack.resize(need, Value::Nil);
                    }
                    for i in 0..count {
                        let v = if i >= n {
                            Value::Nil
                        } else if let Some(t) = vt {
                            t.get_int(i as i64 + 1)
                        } else {
                            self.stack[(func_slot + 1 + i) as usize]
                        };
                        self.stack[(abs_a + i) as usize] = v;
                    }
                    if wanted < 0 {
                        self.top = abs_a + count;
                    }
                }
                Op::GetVarg => {
                    // materialize the vararg table (PUC table.pack shape) from the
                    // stack varargs — used when the named vararg is written /
                    // escapes / is `_ENV`. It is kept BOTH in func_slot (so `...`
                    // sees later writes) and in the local register R[A].
                    let n = n_varargs;
                    let t = self.heap.new_table();
                    {
                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                        let tm = unsafe { t.as_mut() };
                        for i in 0..n {
                            let _ = tm.set_int(
                                &mut self.heap,
                                i as i64 + 1,
                                self.stack[(func_slot + 1 + i) as usize],
                            );
                        }
                    }
                    let n_key = Value::Str(self.heap.intern(b"n"));
                    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                    unsafe { t.as_mut() }
                        .set(&mut self.heap, n_key, Value::Int(n as i64))
                        .expect("'n' is a valid key");
                    // once-per-table barrier (mirror SETLIST): t is born BLACK
                    // during Propagate; the bulk inserts above don't barrier.
                    self.heap
                        .barrier_back(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
                    self.stack[func_slot as usize] = Value::Table(t);
                    self.set_r(base, inst.a(), Value::Table(t));
                }
                Op::ExtraArg => unreachable!("EXTRAARG executed directly"),
                op => unreachable!("{op:?} is run by the fast loop"),
            }
            // the loop head reloads everything from the frame
        }
    }

    #[inline(always)]
    fn pc_of_top(&self) -> u32 {
        self.top_frame().pc
    }

    #[inline(always)]
    fn bump_pc(&mut self) {
        // Inline `top_frame_mut`: top is guaranteed Lua (continuation frames
        // drained at dispatch loop head). Avoids the and_then/lua_mut Option
        // layers — bump_pc fires per Jmp / cond_skip miss, so the savings add
        // up over `fib_28`'s ~500k jumps.
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(f) => f.pc += 1,
            _ => unreachable!("Cont frame at bump_pc"),
        }
    }

    #[inline(always)]
    fn add_pc(&mut self, d: i32) {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
            CallFrame::Lua(f) => f.pc = (f.pc as i64 + d as i64) as u32,
            _ => unreachable!("Cont frame at add_pc"),
        }
    }

    /// PUC conditional-skip convention: the JMP that follows is executed when
    /// `cond == k`; otherwise it is skipped.
    #[inline(always)]
    fn cond_skip(&mut self, cond: bool, k: bool) {
        if cond != k {
            self.bump_pc();
        }
    }

    // ---- indexing (with __index/__newindex chains) ----

    /// The `#` length operation: string byte length, `__len` if present, else
    /// the raw table border. Returns the raw length value (may be non-integer
    /// when `__len` is exotic).
    pub(crate) fn len_value(&mut self, v: Value) -> Result<Value, LuaError> {
        match self.len_step(v)? {
            MmOut::Done(n) => Ok(n),
            // PUC calls unary metamethods with the operand twice
            MmOut::Mm { func, recv } => self.call_mm1(func, &[recv, recv]),
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from len_step"),
        }
    }

    /// Decide equality, or surface the `__eq` metamethod to call. `Done` carries
    /// the boolean result; `Mm` (when raw equality fails and both are tables
    /// with an `__eq`) carries the metamethod — called with `(l, r)`.
    fn eq_step(&mut self, l: Value, r: Value) -> MmOut {
        if l.raw_eq(r) {
            return MmOut::Done(Value::Bool(true));
        }
        if let (Value::Table(_), Value::Table(_)) | (Value::Userdata(_), Value::Userdata(_)) =
            (l, r)
        {
            // PUC 5.3+ accepts any `__eq` reachable from either operand; 5.1
            // and 5.2 require the two operands' metatables to expose the same
            // `__eq` (`get_compTM` / `get_equalTM`) — `c == d` where `d` has
            // no metatable falls straight back to raw inequality. events.lua
            // 5.1 :262 bakes this in.
            let mm = if self.version() <= LuaVersion::Lua52 {
                self.get_comp_mm(l, r, Mm::Eq)
            } else {
                let mut m = self.get_mm(l, Mm::Eq);
                if m.is_nil() {
                    m = self.get_mm(r, Mm::Eq);
                }
                m
            };
            if !mm.is_nil() {
                return MmOut::Mm { func: mm, recv: l };
            }
        }
        MmOut::Done(Value::Bool(false))
    }

    // ---- arithmetic ----

    // ---- comparison ----

    /// `lua_compare(L, a, b, LUA_OPEQ)`: equality including `__eq`.
    pub(crate) fn equal(&mut self, l: Value, r: Value) -> Result<bool, LuaError> {
        match self.eq_step(l, r) {
            MmOut::Done(v) => Ok(v.truthy()),
            MmOut::Mm { func, .. } => Ok(self.call_mm1(func, &[l, r])?.truthy()),
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from eq_step"),
        }
    }

    pub(crate) fn less_than(&mut self, l: Value, r: Value, or_eq: bool) -> Result<bool, LuaError> {
        match self.less_step(l, r, or_eq)? {
            MmOut::Done(v) => Ok(v.truthy()),
            MmOut::Mm { func, .. } => Ok(self.call_mm1(func, &[l, r])?.truthy()),
            MmOut::CompareSynth { func } => {
                // ≤5.3 `__le` via `not __lt(r, l)`. Synchronous helper used
                // by library code (sort comparator etc.) — no yield expected
                // here (a yield would have hit `call_noyield`'s C boundary).
                Ok(!self.call_mm1(func, &[r, l])?.truthy())
            }
        }
    }

    /// Decide `l < r` / `l <= r`, or surface the `__lt`/`__le` metamethod. `Done`
    /// carries the boolean result; `Mm` (for non-number/string operands) carries
    /// the metamethod — called with `(l, r)`; raises the PUC compare error when
    /// neither operand provides one.
    fn less_step(&mut self, l: Value, r: Value, or_eq: bool) -> Result<MmOut, LuaError> {
        let b = match (l, r) {
            (Value::Int(a), Value::Int(b)) => {
                if or_eq {
                    a <= b
                } else {
                    a < b
                }
            }
            (Value::Float(a), Value::Float(b)) => {
                if or_eq {
                    a <= b
                } else {
                    a < b
                }
            }
            (Value::Int(a), Value::Float(b)) => {
                if or_eq {
                    int_le_float(a, b)
                } else {
                    int_lt_float(a, b)
                }
            }
            (Value::Float(a), Value::Int(b)) => {
                if a.is_nan() {
                    false
                } else if or_eq {
                    !int_lt_float(b, a)
                } else {
                    !int_le_float(b, a)
                }
            }
            (Value::Str(a), Value::Str(b)) => {
                let (a, b) = (a.as_bytes(), b.as_bytes());
                if or_eq { a <= b } else { a < b }
            }
            (l, r) => {
                let event = if or_eq { Mm::Le } else { Mm::Lt };
                // PUC 5.1's `get_compTM` rule applies to ordered comparisons
                // too: both operands' metatables must expose the same
                // implementation for `__lt` / `__le` to fire. events.lua 5.1
                // :262 expects `c < d` (where `d` has no metatable) to error
                // with the default "attempt to compare two table values"
                // rather than running c's `__lt` blindly.
                let mm = if self.version() <= LuaVersion::Lua51 {
                    self.get_comp_mm(l, r, event)
                } else {
                    let mut m = self.get_mm(l, event);
                    if m.is_nil() {
                        m = self.get_mm(r, event);
                    }
                    m
                };
                // PUC ≤5.4: `a <= b` falls back to `not (b < a)` when neither
                // operand carries `__le` (5.4 through its default build's
                // LUA_COMPAT_LT_LE); 5.5 requires an explicit `__le`.
                // events.lua 5.2/5.3 :172 relies on the synthesis — its
                // metatable defines only `__lt`. The `__lt` is looked up as
                // for `b < a`: on `b` first (5.1: the same one on both). The
                // fallback calls `__lt(r, l)` synchronously (the suite's
                // `__lt` doesn't yield) and negates the result; the yieldable
                // `__lt` path stays reserved for the explicit `<` operator.
                if mm.is_nil() && or_eq && self.version < LuaVersion::Lua55 {
                    let mm_lt = if self.version <= LuaVersion::Lua51 {
                        self.get_comp_mm(r, l, Mm::Lt)
                    } else {
                        let m = self.get_mm(r, Mm::Lt);
                        if m.is_nil() {
                            self.get_mm(l, Mm::Lt)
                        } else {
                            m
                        }
                    };
                    if !mm_lt.is_nil() {
                        return Ok(MmOut::CompareSynth { func: mm_lt });
                    }
                }
                if mm.is_nil() {
                    // PUC luaG_ordererror: "two X values" when the operand
                    // types match, "X with Y" otherwise (objtypename-aware).
                    let (t1, t2) = (self.obj_typename(l), self.obj_typename(r));
                    return Err(self.runerror(&if t1 == t2 {
                        format!("attempt to compare two {t1} values")
                    } else {
                        format!("attempt to compare {t1} with {t2}")
                    }));
                }
                return Ok(MmOut::Mm { func: mm, recv: l });
            }
        };
        Ok(MmOut::Done(Value::Bool(b)))
    }

    // ---- numeric for ----

    /// Check and convert a numeric for's control values the way the
    /// dialect's `OP_FORPREP` does. The integer loop is chosen by the
    /// values' tags (a numeric string makes it a float loop, 5.3+); the
    /// check order, wording and the zero-step error differ per version:
    /// 5.1/5.2 test initial value, limit, step; 5.3+ limit, step, initial
    /// value; only 5.4+ reject a zero step, and an integer loop does that
    /// before looking at the limit.
    fn for_operands(&mut self, base: u32, a: u32) -> Result<(Num, Num, Num), LuaError> {
        let (init, limit, step) = (self.r(base, a), self.r(base, a + 1), self.r(base, a + 2));
        let v = self.version();
        let order = if v <= LuaVersion::Lua52 {
            [("initial value", init), ("limit", limit), ("step", step)]
        } else {
            [("limit", limit), ("step", step), ("initial value", init)]
        };
        if v >= LuaVersion::Lua54 && matches!((init, step), (Value::Int(_), Value::Int(0))) {
            return Err(self.rt_err("'for' step is zero"));
        }
        for (what, val) in order {
            if as_num(val, v).is_none() {
                return Err(self.rt_err(&if v >= LuaVersion::Lua54 {
                    format!(
                        "bad 'for' {what} (number expected, got {})",
                        self.obj_typename(val)
                    )
                } else {
                    format!("'for' {what} must be a number")
                }));
            }
        }
        let n = |val| as_num(val, v).expect("checked above");
        let int_loop =
            v <= LuaVersion::Lua52 || matches!((init, step), (Value::Int(_), Value::Int(_)));
        if int_loop {
            return Ok((n(init), n(limit), n(step)));
        }
        let (i, l, st) = (n(init).as_f64(), n(limit).as_f64(), n(step).as_f64());
        if v >= LuaVersion::Lua54 && st == 0.0 {
            return Err(self.rt_err("'for' step is zero"));
        }
        Ok((Num::Float(i), Num::Float(l), Num::Float(st)))
    }

    fn for_prep(&mut self, inst: Inst, base: u32) -> Result<(), LuaError> {
        let a = inst.a();
        let (init_n, limit_n, step_n) = self.for_operands(base, a)?;
        // PUC 5.1–5.3 `OP_FORPREP` stores `i = init - step` and *unconditionally*
        // jumps to the matching `OP_FORLOOP` — the body never runs ahead of the
        // first test, so each successful iteration emits a backward `OP_FORLOOP`
        // jump (db.lua's `for i=1,4 do a=1 end` ↦ 5 line-hook events instead of
        // 5.4's 4). 5.4+ collapsed that to a count-based fall-through. The skip
        // distance in luna's encoding is `loop_pc - prep_pc`; firing
        // `add_pc(bx - 1)` lands the running pc on OP_FORLOOP itself.
        let pre53 = self.version() <= LuaVersion::Lua53;
        match (init_n, step_n) {
            (Num::Int(i0), Num::Int(st)) => {
                if pre53 {
                    // PUC 5.3 `forlimit`: int limit passes through; float limit
                    // gets clamped to MIN/MAX with a `stopnow` flag set only
                    // when the clamp is unreachable (positive float with a
                    // negative step → limit=MAX, stopnow; negative float with
                    // step>=0 → limit=MIN, stopnow). On `stopnow` PUC rewrites
                    // `init = 0` so OP_FORLOOP's first test against the
                    // unreachable clamp fails cleanly. An ordinary in-range
                    // empty loop (e.g. `for i = 1, 0`) is *not* `stopnow` — it
                    // lets OP_FORLOOP's natural test reject the first step.
                    let (lim, stopnow) = match limit_n {
                        Num::Int(l) => (l, false),
                        Num::Float(f) => {
                            // `luaV_tointeger` floors (ceils for a negative
                            // step); a float it cannot fit is clamped on
                            // the side of its sign, NaN counting as
                            // negative (`0 < n` is false).
                            let conv = if st < 0 { f.ceil() } else { f.floor() };
                            if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0)
                                .contains(&conv)
                            {
                                (conv as i64, false)
                            } else if f > 0.0 {
                                (i64::MAX, st < 0)
                            } else {
                                (i64::MIN, st > 0)
                            }
                        }
                    };
                    let initv = if stopnow { 0 } else { i0 };
                    let pre = initv.wrapping_sub(st);
                    self.set_r(base, a, Value::Int(pre));
                    self.set_r(base, a + 1, Value::Int(lim));
                    self.set_r(base, a + 2, Value::Int(st));
                    self.add_pc(inst.bx() as i32 - 1);
                    return Ok(());
                }
                let (lim, empty) = int_for_limit(limit_n, i0, st);
                if empty {
                    self.add_pc(inst.bx() as i32);
                    return Ok(());
                }
                let count = if st > 0 {
                    (lim as u64).wrapping_sub(i0 as u64) / (st as u64)
                } else {
                    (i0 as u64).wrapping_sub(lim as u64) / (st as i128).unsigned_abs() as u64
                };
                self.set_r(base, a, Value::Int(i0));
                self.set_r(base, a + 1, Value::Int(count as i64));
                self.set_r(base, a + 2, Value::Int(st));
                self.set_r(base, a + 3, Value::Int(i0));
            }
            _ => {
                let (x0, lim, st) = (init_n.as_f64(), limit_n.as_f64(), step_n.as_f64());
                if pre53 {
                    let pre = x0 - st;
                    self.set_r(base, a, Value::Float(pre));
                    self.set_r(base, a + 1, Value::Float(lim));
                    self.set_r(base, a + 2, Value::Float(st));
                    self.add_pc(inst.bx() as i32 - 1);
                    return Ok(());
                }
                // lvm.c `forprep`: skip only when `0 < step ? limit < init :
                // init < limit`; a NaN makes both false, so the body runs
                // once (with a NaN step, on the second test's side)
                let skip = if 0.0 < st { lim < x0 } else { x0 < lim };
                let runs = !skip;
                if !runs {
                    self.add_pc(inst.bx() as i32);
                    return Ok(());
                }
                self.set_r(base, a, Value::Float(x0));
                self.set_r(base, a + 1, Value::Float(lim));
                self.set_r(base, a + 2, Value::Float(st));
                self.set_r(base, a + 3, Value::Float(x0));
            }
        }
        Ok(())
    }

    #[inline(always)]
    fn for_loop(&mut self, inst: Inst, base: u32) -> Result<(), LuaError> {
        let a = inst.a();
        // PUC 5.1–5.3 `OP_FORLOOP` compares the post-step `i` to `limit`
        // directly (R[a+1] holds the limit, *not* a remaining-count) so the
        // first iteration's test fires through the same backward-jump path as
        // every later iteration. 5.4+ switched to the count-based form luna
        // already uses for `Int`; the float branch was already PUC-3.x-style.
        let v = self.version();
        let pre53 = v <= LuaVersion::Lua53;
        // `for_prep` leaves the three slots all Int or all Float; anything
        // else was written by `debug.setlocal` or by crafted bytecode. PUC
        // reads such slots unchecked (garbage or a crash); luna raises.
        match (self.r(base, a), self.r(base, a + 1), self.r(base, a + 2)) {
            (Value::Int(cur), Value::Int(lim), Value::Int(st)) if pre53 => {
                let next = cur.wrapping_add(st);
                let cont = if st > 0 { next <= lim } else { next >= lim };
                if cont {
                    self.set_r(base, a, Value::Int(next));
                    self.set_r(base, a + 3, Value::Int(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            // the count is unsigned (PUC `lua_Unsigned`): a loop over
            // more than 2^63 values stores a "negative" one
            (Value::Int(cur), Value::Int(count), Value::Int(st)) => {
                if count != 0 {
                    let next = cur.wrapping_add(st);
                    self.set_r(base, a, Value::Int(next));
                    self.set_r(base, a + 1, Value::Int(count.wrapping_sub(1)));
                    self.set_r(base, a + 3, Value::Int(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            (Value::Float(cur), Value::Float(lim), Value::Float(st)) => {
                self.float_for_step(inst, base, cur, lim, st);
            }
            // 5.1/5.2 have one number type, so a number of the other
            // representation stored into a slot is still a valid state
            (x, l, s) if v <= LuaVersion::Lua52 => {
                match (as_number(x), as_number(l), as_number(s)) {
                    (Some(cur), Some(lim), Some(st)) => {
                        self.float_for_step(inst, base, cur.as_f64(), lim.as_f64(), st.as_f64())
                    }
                    _ => return Err(self.rt_err("'for' state corrupted")),
                }
            }
            _ => return Err(self.rt_err("'for' state corrupted")),
        }
        Ok(())
    }

    #[inline(always)]
    fn float_for_step(&mut self, inst: Inst, base: u32, cur: f64, lim: f64, st: f64) {
        let a = inst.a();
        let next = cur + st;
        let cont = if st > 0.0 { next <= lim } else { next >= lim };
        if cont {
            self.set_r(base, a, Value::Float(next));
            self.set_r(base, a + 3, Value::Float(next));
            self.add_pc(-(inst.bx() as i32));
        }
    }

    // ---- native helpers (used by builtins) ----

    /// A native function's own captured upvalue (self lives at func_slot).
    ///
    /// Public so `native_typed` trampolines and embedders authoring
    /// stateful natives via `native_with(...)` can read their upvals.
    pub fn nat_upval(&self, func_slot: u32, i: usize) -> Value {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("native frame without native closure");
        };
        nc.upvals[i]
    }

    /// Number of upvalues captured by the native at `func_slot` (variadic
    /// captures such as the `io.lines` format list).
    pub(crate) fn nat_upcount(&self, func_slot: u32) -> usize {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("native frame without native closure");
        };
        nc.upvals.len()
    }

    /// Write a native function's own upvalue (stateful iterators).
    pub(crate) fn nat_set_upval(&mut self, func_slot: u32, i: usize, v: Value) {
        let Value::Native(nc) = self.stack[func_slot as usize] else {
            unreachable!("native frame without native closure");
        };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { nc.as_mut() }.upvals[i] = v;
        // NativeClosure.upvals is traced as part of its Trace; a long-lived
        // stateful iterator closure (e.g. string.gmatch) sees many writes —
        // barrier_back once-and-done is cheaper than per-child forward.
        self.heap
            .barrier_back(nc.as_ptr() as *mut crate::runtime::heap::GcHeader);
    }

    /// Read the i-th positional argument inside a `NativeFn` body
    /// (analogous to `lua_tovalue(L, i + 1)`). `i >= nargs` yields `Nil`,
    /// matching PUC's "missing arg is nil" contract. Public so embedders
    /// can author their own natives.
    pub fn nat_arg(&self, func_slot: u32, nargs: u32, i: u32) -> Value {
        if i < nargs {
            self.stack[(func_slot + 1 + i) as usize]
        } else {
            Value::Nil
        }
    }

    /// Overwrite the i-th argument slot of the running native (the in-place
    /// conversion `lua_tolstring` performs on a number argument).
    pub(crate) fn nat_set_arg(&mut self, func_slot: u32, i: u32, v: Value) {
        self.stack[(func_slot + 1 + i) as usize] = v;
    }

    /// Push the return values of a `NativeFn` and return their count
    /// (analogous to pushing N values then `return N` from a C function).
    /// Public so embedders can author their own natives.
    pub fn nat_return(&mut self, func_slot: u32, vals: &[Value]) -> u32 {
        let need = func_slot as usize + vals.len();
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        for (i, &v) in vals.iter().enumerate() {
            self.stack[func_slot as usize + i] = v;
        }
        vals.len() as u32
    }

    /// Fast string concatenation of an adjacent pair, or `None` when a
    /// `__concat` metamethod is required.
    fn concat_pair(&mut self, l: Value, r: Value) -> Result<Option<Value>, LuaError> {
        let legacy = self.float_fmt();
        // Length-check fast paths for both string operands BEFORE the
        // (expensive) copy in `concat_piece`, so a runaway `a..a..a..…`
        // chain (5.1 big.lua / 5.5 heavy.lua's `teststring`) raises the
        // overflow on the first pair that would exceed `INT_MAX` instead
        // of allocating multi-GB intermediates first.
        let max_str = i32::MAX as usize;
        if let (Value::Str(ls), Value::Str(rs)) = (l, r) {
            let a_len = ls.as_bytes().len();
            let b_len = rs.as_bytes().len();
            let new_len = a_len.checked_add(b_len);
            if new_len.is_none() || new_len.unwrap() > max_str {
                return Err(self.rt_err("string length overflow"));
            }
        }
        match (concat_piece(l, legacy), concat_piece(r, legacy)) {
            (Some(a), Some(b)) => {
                // PUC `MAX_SIZE` for Lua strings is `INT_MAX`; an attempt to
                // concat past it raises "string length overflow"
                // (5.5 heavy.lua `teststring` doubles `a..a..…` until it hits
                // exactly this wall).
                let new_len = a.len().checked_add(b.len());
                if new_len.is_none() || new_len.unwrap() > max_str {
                    return Err(self.rt_err("string length overflow"));
                }
                let mut combined = a;
                combined.extend_from_slice(&b);
                Ok(Some(Value::Str(self.heap.intern(&combined))))
            }
            _ => Ok(None),
        }
    }

    /// Fold the concat operands occupying `[base_a .. self.top)` right-to-left
    /// into a single result at `base_a` (PUC `luaV_concat`). Returns after
    /// either finishing (result at `base_a`) or arming a yieldable `__concat`
    /// call — its `Meta` continuation re-enters here on the metamethod's return.
    fn concat_run(&mut self, base_a: u32) -> Result<(), LuaError> {
        // Sum the lengths of all all-Str operands BEFORE starting the
        // right-associative fold so a 129-operand `a..a..…` chain
        // (5.1 big.lua's `rep129(longs)`) raises overflow immediately,
        // not after dozens of multi-GB intermediate intern+hash rounds.
        // A non-Str operand falls through to the per-pair check.
        let max_str = i32::MAX as usize;
        let mut total: usize = 0;
        let mut all_str = true;
        for slot in base_a..self.top {
            match self.stack[slot as usize] {
                Value::Str(s) => match total.checked_add(s.as_bytes().len()) {
                    Some(t) if t <= max_str => total = t,
                    _ => return Err(self.rt_err("string length overflow")),
                },
                _ => {
                    all_str = false;
                    break;
                }
            }
        }
        let _ = all_str; // discrimination already captured by early returns above
        while self.top.saturating_sub(base_a) >= 2 {
            let i = self.top - 1; // rightmost operand
            let x = self.stack[(i - 1) as usize];
            let y = self.stack[i as usize];
            match self.concat_pair(x, y)? {
                Some(s) => {
                    self.stack[(i - 1) as usize] = s;
                    self.top = i; // consumed y
                }
                None => {
                    let mut mm = self.get_mm(x, Mm::Concat);
                    if mm.is_nil() {
                        mm = self.get_mm(y, Mm::Concat);
                    }
                    if mm.is_nil() {
                        let legacy = self.float_fmt();
                        let bad = if concat_piece(x, legacy).is_none() {
                            x
                        } else {
                            y
                        };
                        return Err(self.type_err("concatenate", bad));
                    }
                    // result lands at i-1, dropping y (top→i); resume continues.
                    let dst = i - 1;
                    self.begin_meta_call(mm, &[x, y], MetaAction::Concat { dst, base_a })?;
                    return Ok(());
                }
            }
        }
        self.maybe_collect_garbage(base_a + 1);
        Ok(())
    }

    /// `luaL_tolstring`: `__tostring` (whose result must be a string or a
    /// number, rendered), else the basic rendering, where 5.3+ names a value
    /// by a string `__name` metafield.
    pub fn tostring_value(&mut self, v: Value) -> Result<Vec<u8>, LuaError> {
        let mm = self.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            // `luaL_callmeta` is a plain `lua_call`: `__tostring` cannot yield.
            let r = self.call_noyield(mm, &[v])?;
            return match r.first().copied().unwrap_or(Value::Nil) {
                Value::Str(s) => Ok(s.as_bytes().to_vec()),
                r @ (Value::Int(_) | Value::Float(_)) => Ok(self.tostring_basic(r)),
                // luaL_error: positioned at whatever called the library function
                _ => Err(crate::vm::builtins::raise_str(
                    self,
                    "'__tostring' must return a string",
                )),
            };
        }
        if self.version >= LuaVersion::Lua53
            && !matches!(
                v,
                Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_)
            )
            && let Value::Str(name) = self.get_mm(v, Mm::Name)
        {
            let basic = self.tostring_basic(v);
            let at = basic
                .iter()
                .position(|&c| c == b':')
                .expect("an object renders as `kind: address`");
            let mut out = name.as_bytes().to_vec();
            out.extend_from_slice(&basic[at..]);
            return Ok(out);
        }
        Ok(self.tostring_basic(v))
    }

    /// The dialect's float-rendering flavor: ≤5.2 %.14g
    /// bare, 5.3/5.4 %.14g + ".0", 5.5 two-stage %.15g/%.17g + ".0".
    pub(crate) fn float_fmt(&self) -> numeric::FloatFmt {
        use crate::version::LuaVersion::*;
        match self.version {
            Lua51 | Lua52 => numeric::FloatFmt::Legacy14,
            Lua53 | Lua54 => numeric::FloatFmt::G14,
            _ => numeric::FloatFmt::TwoStage55,
        }
    }

    /// Basic tostring (no metamethods).
    pub(crate) fn tostring_basic(&mut self, v: Value) -> Vec<u8> {
        match v {
            Value::Nil => b"nil".to_vec(),
            Value::Bool(true) => b"true".to_vec(),
            Value::Bool(false) => b"false".to_vec(),
            Value::Int(i) => numeric::num_to_string(Num::Int(i)).into_bytes(),
            // PUC ≤5.2 has no integer subtype — `tostring(2.0)` is `"2"`, not
            // `"2.0"`. The 5.3+ split needs the suffix so `print(2.0)` is
            // distinguishable from `print(2)`. pm.lua :13 builds patterns by
            // concatenating these renderings.
            Value::Float(f) => {
                numeric::num_to_string_for(Num::Float(f), self.float_fmt()).into_bytes()
            }
            Value::Str(s) => s.as_bytes().to_vec(),
            Value::Table(t) => format!("table: {:p}", t.as_ptr()).into_bytes(),
            Value::Closure(c) => format!("function: {:p}", c.as_ptr()).into_bytes(),
            Value::Native(n) => format!("function: {:p}", n.as_ptr()).into_bytes(),
            Value::Coro(co) => format!("thread: {:p}", co.as_ptr()).into_bytes(),
            // PUC names file handles `file (0x…)`; a bare userdata is
            // `userdata: 0x…`. The io library overrides this via __tostring.
            Value::Userdata(u) => format!("userdata: {:p}", u.as_ptr()).into_bytes(),
            // PUC `lua_topointer`/tostring on light udata: "userdata: 0x…"
            // (the "light" qualifier only appears in `luaL_typeerror`).
            Value::LightUserdata(p) => format!("userdata: {p:p}").into_bytes(),
        }
    }
}

impl Vm {
    /// PUC's debug-API placeholder for an unnamed vararg slot returned by
    /// `debug.getlocal(_, -n)`. 5.2/5.3 spelled it `"(*vararg)"`; 5.4
    /// dropped the asterisk in favour of `"(vararg)"`. db.lua 5.2 :189 /
    /// 5.3 :195 / 5.4 :286 baseline on their respective form.
    pub(crate) fn vararg_locvar_name(&self) -> &'static str {
        if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
            "(*vararg)"
        } else {
            "(vararg)"
        }
    }

    /// PUC's debug-API placeholder for an unnamed temporary on a C
    /// activation. 5.2/5.3 reported `"(*temporary)"`; 5.4 switched to
    /// `"(C temporary)"`. db.lua 5.2 :288, 5.3 :312, 5.4 :404 each pin
    /// their spelling.
    pub(crate) fn temporary_locvar_name(&self) -> &'static str {
        if matches!(
            self.version,
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53
        ) {
            // PUC 5.1's `findlocal` C-frame branch reported `(*temporary)`
            // (db.lua :228 pins it). 5.2/5.3 kept the spelling, 5.4 changed
            // to `(C temporary)`.
            "(*temporary)"
        } else {
            "(C temporary)"
        }
    }

    /// PUC's debug-API placeholder for an unnamed Lua-frame temporary
    /// (an arithmetic intermediate sitting past the last named local on a
    /// live register slot). 5.2/5.3 reported `"(*temporary)"`; 5.4 dropped
    /// the asterisk to `"(temporary)"`. db.lua 5.3 :786, 5.4 :966 pin the
    /// spelling.
    pub(crate) fn lua_temporary_locvar_name(&self) -> &'static str {
        if matches!(
            self.version,
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53
        ) {
            "(*temporary)"
        } else {
            "(temporary)"
        }
    }

    /// PUC `pushglobalfuncname`: walk `package.loaded` to depth 2 looking for a
    /// native whose function pointer matches `target`, and return its qualified
    /// name (e.g. `"table.sort"`). A `_G.X` match is stripped to `"X"`. Returns
    /// `None` if no match is found. Used by `arg_error` when the running native
    /// was invoked from another native (PUC `ar.name == NULL` at level 0).
    pub(crate) fn pushglobalfuncname(
        &mut self,
        target: crate::runtime::value::NativeFn,
    ) -> Option<String> {
        let pkg_k = Value::Str(self.heap.intern(b"package"));
        let pkg = match self.globals().get(pkg_k) {
            Value::Table(t) => t,
            _ => return None,
        };
        let loaded_k = Value::Str(self.heap.intern(b"loaded"));
        let loaded = match pkg.get(loaded_k) {
            Value::Table(t) => t,
            _ => return None,
        };
        let matches = |v: Value| -> bool {
            matches!(v, Value::Native(nc) if std::ptr::fn_addr_eq(nc.f, target))
        };
        let mut k = Value::Nil;
        while let Ok(Some((nk, nv))) = loaded.next(k) {
            k = nk;
            let Value::Str(outer) = nk else { continue };
            let outer = String::from_utf8_lossy(outer.as_bytes()).into_owned();
            if matches(nv) {
                return Some(if outer == "_G" { String::new() } else { outer });
            }
            if let Value::Table(inner_t) = nv {
                let mut k2 = Value::Nil;
                while let Ok(Some((nk2, nv2))) = inner_t.next(k2) {
                    k2 = nk2;
                    if matches(nv2)
                        && let Value::Str(inner) = nk2
                    {
                        let inner = String::from_utf8_lossy(inner.as_bytes()).into_owned();
                        return Some(if outer == "_G" {
                            inner
                        } else {
                            format!("{outer}.{inner}")
                        });
                    }
                }
            }
        }
        None
    }

    /// How the caller named the running native (PUC `lua_getinfo("n")` at
    /// level 0): `None` when it gives no name, as when the caller is C.
    pub(crate) fn running_call_name(&self) -> Option<(&'static str, String)> {
        let ts = self.thread_stack(None);
        if ts.levels.is_empty() {
            return None;
        }
        self.level_name(&ts, 0)
    }

    /// Read an upvalue cell of a closure (debug.getupvalue).
    pub(crate) fn upvalue_value(&self, cl: Gc<LuaClosure>, idx: usize) -> Value {
        match cl.upvals()[idx].state() {
            UpvalState::Open { slot, thread } => self.read_slot(slot, thread),
            UpvalState::Closed(v) => v,
        }
    }

    /// Write an upvalue cell of a closure (debug.setupvalue).
    pub(crate) fn upvalue_set_value(&mut self, cl: Gc<LuaClosure>, idx: usize, v: Value) {
        let uv = cl.upvals()[idx];
        match uv.state() {
            UpvalState::Open { slot, thread } => self.write_slot(slot, thread, v),
            UpvalState::Closed(_) => {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { uv.as_mut() }.set_closed(v);
                self.heap
                    .barrier_forward(uv.as_ptr() as *mut crate::runtime::heap::GcHeader, v);
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// AOT trace dispatch install.
//
// The deploy-side resolver in `luna-runtime-helpers` walks the binary's
// trace-meta section after `vm.load`, resolves each entry's
// `(proto_hash, head_pc, fn_ptr)` triple against the loaded chunk's
// proto tree, and pushes a `CompiledTrace` onto the matching Proto's
// `traces` Vec via [`Vm::install_aot_trace`] below. The existing
// trace-dispatch loop (this file's `cl.proto.traces.borrow().iter()
// .find(|t| t.head_pc == pc && t.dispatchable)`) then fires the AOT
// mcode without further plumbing — same code path the runtime JIT
// uses.
//
// Why a separate impl block: keeps the AOT API surface (one fn) easy
// to locate when grep'ing for `install_aot_trace`, without dragging
// the 8500-line `impl Vm` block above.
// ────────────────────────────────────────────────────────────────────

impl Vm {
    /// Install a precompiled
    /// `CompiledTrace` onto `proto.traces` so the interp dispatcher
    /// fires it at the trace's `head_pc`. This is the runtime install
    /// API the deploy-side `luna-runtime-helpers` resolver calls once
    /// per AOT-emitted trace meta entry, after looking up `proto` by
    /// stable hash (see `crate::runtime::function::Proto::stable_hash`).
    ///
    /// # What this does
    ///
    /// Pushes `trace` onto `proto.traces` via the existing `RefCell`.
    /// The trace's `entry` fn ptr must already point at runnable
    /// machine code (the AOT linker resolved the symbol at link time;
    /// the deploy resolver passes the address verbatim).
    ///
    /// # What this does NOT do
    ///
    /// - **No deduplication.** Calling twice with the same `head_pc`
    ///   pushes two entries; the dispatcher's `find` will pick the
    ///   first match. The deploy resolver is responsible for not
    ///   double-installing.
    /// - **No invalidation of the runtime JIT cache.** If the runtime
    ///   JIT later records + compiles a trace for the same
    ///   `(proto, head_pc)`, both coexist on `proto.traces` and the
    ///   dispatcher's `find` picks whichever appears first. AOT
    ///   traces install before any runtime recording is possible
    ///   (resolver runs before `vm.load` returns its first closure),
    ///   so AOT traces win the race for the same site.
    /// - **No coverage gating.** AOT traces are trusted by
    ///   construction — they were validated at compile time. Setting
    ///   `dispatchable: false` on the input would silently disable
    ///   dispatch; the caller controls that flag.
    ///
    /// # Safety / soundness
    ///
    /// `trace.entry` is an `unsafe extern "C" fn` (mmap'd or linked
    /// machine code). Soundness contract:
    ///
    /// - The fn pointer must remain valid for the `Vm`'s lifetime.
    ///   In the AOT-binary deploy shape this is trivially satisfied —
    ///   the fn lives in the binary's `.text`.
    /// - `trace.entry_tags` / `exit_tags` / `window_size` must match
    ///   what the trace's IR actually compiled against; the dispatcher
    ///   uses them to marshal `reg_state` in and out without further
    ///   validation. A mismatch corrupts vm.stack.
    ///
    /// The AOT pipeline (`luna-aot`) is responsible for ensuring these
    /// invariants hold; this fn is a plain push — no validation that
    /// would slow the dispatcher's hot path either.
    pub fn install_aot_trace(
        &mut self,
        proto: crate::runtime::Gc<crate::runtime::function::Proto>,
        trace: crate::jit::trace::CompiledTrace,
    ) {
        let _ = self; // resolver passes &mut Vm for symmetry with future
        // pending-install + hash-walk variants; nothing on `self` to
        // mutate today because the install target lives on the Proto.
        cache_trace(proto, trace);
    }

    /// Walk the proto tree
    /// reachable from `root` and return `(proto, stable_hash)` pairs
    /// for every Proto found. Used by the deploy-side resolver to
    /// match AOT-emitted `proto_hash` keys against the freshly
    /// `undump`'d chunk's protos.
    ///
    /// The walk is BFS over `Proto.protos`. Same-Proto deduplication
    /// is done via `Gc::as_ptr` identity — a Proto re-referenced from
    /// multiple nested closures (rare; the cache field would catch
    /// the closure-side dedup, not the Proto side) is reported once.
    ///
    /// # Why on `&Vm` and not a free fn
    ///
    /// Keeps the AOT install API discoverable on the Vm surface —
    /// `vm.collect_proto_hashes(root)` reads naturally next to
    /// `vm.install_aot_trace(proto, trace)`. Doesn't actually touch
    /// any Vm field, so `&self` (read-only) is enough.
    pub fn collect_proto_hashes(
        &self,
        root: crate::runtime::Gc<crate::runtime::function::Proto>,
    ) -> Vec<(
        crate::runtime::Gc<crate::runtime::function::Proto>,
        [u8; 16],
    )> {
        let _ = self;
        let mut out = Vec::new();
        let mut seen: std::collections::HashSet<*const crate::runtime::function::Proto> =
            std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<
            crate::runtime::Gc<crate::runtime::function::Proto>,
        > = std::collections::VecDeque::new();
        queue.push_back(root);
        while let Some(p) = queue.pop_front() {
            let key = p.as_ptr() as *const _;
            if !seen.insert(key) {
                continue;
            }
            out.push((p, p.stable_hash()));
            for &child in p.protos.iter() {
                queue.push_back(child);
            }
        }
        out
    }
}

/// Recordings of one trace head that may fail to compile, or overflow the
/// recorder, before the head is no longer recorded (LuaJIT likewise
/// blacklists a trace start after repeated failures). A few tries, since a
/// later recording can see different register kinds or take a shorter path.
const MAX_TRACE_COMPILE_FAILURES: u8 = 3;

fn note_trace_compile_failure(proto: Gc<crate::runtime::function::Proto>, head_pc: u32) {
    let mut failures = proto.trace_compile_failures.borrow_mut();
    let n = match failures.iter_mut().find(|(pc, _)| *pc == head_pc) {
        Some((_, n)) => {
            *n = n.saturating_add(1);
            *n
        }
        None => {
            failures.push((head_pc, 1));
            1
        }
    };
    if head_pc == 0 && n >= MAX_TRACE_COMPILE_FAILURES {
        proto.trace_call_head_settled.set(true);
    }
}

/// Park `ct` on `proto.traces`, keeping `has_dispatchable_trace` in step
/// with the dispatcher's admit test.
fn cache_trace(proto: Gc<crate::runtime::function::Proto>, ct: crate::jit::trace::CompiledTrace) {
    if ct.dispatchable || ct.downrec_link.is_some() {
        proto.has_dispatchable_trace.set(true);
        use crate::runtime::function::{TRACE_HEADS_MANY, TRACE_HEADS_NONE};
        let mut heads = proto.trace_heads.get();
        if heads[0] == TRACE_HEADS_NONE || heads[0] == ct.head_pc {
            heads[0] = ct.head_pc;
        } else if heads[1] == TRACE_HEADS_NONE || heads[1] == ct.head_pc {
            heads[1] = ct.head_pc;
        } else {
            heads = [TRACE_HEADS_MANY; 2];
        }
        proto.trace_heads.set(heads);
    }
    if ct.head_pc == 0 {
        proto.trace_call_head_settled.set(true);
    }
    proto.traces.borrow_mut().push(TArc::new(ct));
}

fn trace_head_abandoned(proto: Gc<crate::runtime::function::Proto>, head_pc: u32) -> bool {
    proto
        .trace_compile_failures
        .borrow()
        .iter()
        .any(|&(pc, n)| pc == head_pc && n >= MAX_TRACE_COMPILE_FAILURES)
}
