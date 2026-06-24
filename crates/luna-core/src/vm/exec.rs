//! The interpreter. Dispatch is a plain match over opcodes (the P10 ceiling
//! pass owns dispatch optimization). Lua→Lua calls share one loop and never
//! recurse the Rust stack; only native↔Lua boundaries do (e.g. pcall).
//!
//! Varargs follow 5.5 semantics: a vararg call materializes a vararg table
//! (fields 1..n plus "n") kept in the function's own stack slot; `...`
//! expands from it and `...name` binds it. 5.1 LUAI_COMPAT_VARARG also
//! materializes a local `arg` table (see `proto.has_compat_vararg_arg`).

use crate::compiler::compile_chunk;
use crate::frontend::{SyntaxError, parse};
use crate::numeric::{self, Num};
use crate::runtime::heap::GcHeader;
use crate::runtime::{
    AfterClose, CallFrame, CloseCont, ContKind, Coro, CoroStatus, Frame, Gc, Heap, LuaClosure,
    MetaAction, MetaCont, NativeClosure, NativeCont, Table, TableError, UpvalState, Upvalue, Value,
};
use crate::version::LuaVersion;
use crate::vm::builtins::{nat_pairs, nat_pcall, nat_xpcall};
use crate::vm::error::LuaError;
use crate::vm::isa::{Inst, Op};

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
/// A future `feature = "send"` (post-v1.1 sprint) will gate an
/// opt-in `Arc<RwLock<T>>` mode with a hard ≤8% perf regression
/// budget. See `.dev/rfcs/v1.1-rfc-vm-send-sync.md` for the design.
pub struct Vm {
    /// The GC heap owned by this VM. Embedders normally interact via the
    /// `Vm` methods (`load` / `call_value` / `set_global` / …) rather than
    /// the heap directly.
    pub heap: Heap,
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    /// P17-D Week 1 shadow — frames_top mirrors `self.frames.len()`.
    /// Synced on every push/pop in `frames_push_sync`/`frames_pop_sync`
    /// helpers (debug-asserted on use). NOT consumed by readers yet;
    /// week 1 is pure scaffold. Week 2-N migrations replace readers
    /// one slice at a time, then remove `frames: Vec<CallFrame>` in
    /// favour of a flat `[CallFrame; MAX_FRAMES]` indexed by frames_top.
    frames_top: u32,
    /// open upvalues, sorted ascending by stack slot
    open_upvals: Vec<(u32, Gc<Upvalue>)>,
    /// to-be-closed slots, ascending
    tbc: Vec<u32>,
    /// logical stack top for multi-result sequences
    pub(crate) top: u32,
    globals: Gc<Table>,
    /// shared metatable for all strings (populated by the string lib, P04)
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
    msgh_depth: u32,
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
    /// `None` while the main thread runs (P05)
    current: Option<Gc<crate::runtime::Coro>>,
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
    /// P09 embedding cooperative budget: a per-Vm tick counter that the run
    /// loop decrements once per dispatch turn. When it hits zero the loop
    /// raises a catchable "instruction budget exceeded" error so the embedder
    /// can yield control back to its caller (short-script eval, game
    /// frame budgets). `None` = unbounded; reset on each call via
    /// `set_instr_budget`.
    pub(crate) instr_budget: Option<i64>,
    // v1.1 A2 — JIT-specific fields moved to `JitState` sidecar; see
    // `self.jit` below + `crate::vm::jit_state` for field docs.
    // (Was: jit_enabled here.)
    // v1.1 A2 — was: trace_jit_enabled (moved to JitState).
    // v1.1 A2 — was: p16_self_link_enabled (moved to JitState).
    // v1.1 A2 — was: active_trace, recording_frame_base, trace_max_depth_seen,
    // trace_closed_count, trace_aborted_count, trace_inline_abort_count,
    // trace_dispatch_off_reasons, trace_compile_failed_reasons, trace_closed_lens,
    // trace_compiled_count, trace_compile_failed_count, trace_dispatched_count,
    // trace_deopt_count, trace_side_trace_{started,compiled,shape_mismatch}_count,
    // trace_{sinkable,accum_bufferable}_seen_count, trace_{sunk_alloc,
    // materialize_emit,closure_emit}_count — all moved to JitState.
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
    /// through the per-dialect PUC translators in `crate::vm::dump::puc`
    /// (Phase LB Wave 2 — currently returns "not yet implemented" stubs).
    /// Embedder toggles via `set_puc_bytecode_loading`.
    pub(crate) puc_bytecode_loading: bool,
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
    /// the running thread's debug hook state (`debug.sethook`); per-thread,
    /// swapped with the execution context on a coroutine resume/yield
    pub(crate) hook: HookState,
    /// true while the hook itself runs, so its own execution fires no events
    /// (PUC clears the mask for the duration)
    pub(crate) in_hook: bool,
    /// arms the next Lua frame's `tailcalls` count (PUC `ci->u.l.tailcalls`),
    /// consumed by `push_frame`. `OP_TailCall` sets it to the caller's
    /// own tailcalls + 1 before begin_call so deeply tail-recursive chains
    /// accumulate the count instead of capping at 1.
    pub(crate) pending_tailcalls: u32,
    /// Name of the C native that just propagated an error (captured before
    /// the native is popped from `running_natives`). Lets a dying coroutine
    /// preserve `[C]: in function '<name>'` at the top of its traceback
    /// snapshot — PUC walks `luaG_funcnamefrompc` over a still-live ci, but
    /// luna's native frames are off-stack so we stash the name explicitly.
    pub(crate) errored_native: Option<String>,
    /// PUC `CallInfo.u2.transferinfo`: index of the first transferred value
    /// (relative to the activation's func slot) and the number transferred.
    /// Set just before firing a call/return hook, read by `getinfo("r")`.
    pub(crate) hook_ftransfer: u16,
    pub(crate) hook_ntransfer: u16,
    /// metamethod event tag (e.g. "close") to attach to the next Lua frame
    /// pushed by `push_frame`; `close_slots` sets this before calling a
    /// `__close` handler so `debug.traceback` names it "metamethod 'close'"
    /// (PUC `CallInfo.u.l.tm`). Single-shot: `push_frame` consumes it.
    pending_tm: Option<&'static str>,
    /// `true` when the next `push_frame` is the user hook function itself,
    /// so `debug.getinfo(1).namewhat` resolves to `"hook"` (PUC
    /// `CIST_HOOKED`). `run_hook` arms it before dispatching the hook.
    pending_is_hook: bool,
    /// traceback snapshot taken at the error point (the first `unwind` entry
    /// for the in-flight error), so that an `xpcall` msgh — which runs *after*
    /// the failed frames are popped — can still see the error point's stack
    /// via `debug.traceback`. PUC `luaG_errormsg` instead runs msgh with the
    /// stack intact; we approximate by snapshotting the string and letting
    /// `d_traceback` consume it. Cleared on Cont catch and at host-level
    /// `call_value` entry (`public_call_depth == 0`).
    pub(crate) error_traceback: Option<Vec<u8>>,
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
    pub(crate) running_natives: Vec<Gc<NativeClosure>>,
    /// Parallel to `running_natives`: each entry's `(func_slot, nargs)` is
    /// the native's argument-window head and width, so `debug.getlocal`
    /// can index it like PUC's `luaG_findlocal` `(C temporary)` path.
    pub(crate) running_native_slots: Vec<(u32, u32)>,
    // v1.1 A2 — was: jit_pending_err, jit_reg_state_buf, jit_str_buf_pool,
    // jit_str_buf_pool_cap, jit_entry_tags_buf, chunk_compiler,
    // trace_compiler — all moved to JitState. See `jit` below.
    /// v1.1 A2 — JIT sidecar. Always present (never `Option`); inert
    /// when `chunk_compiler` / `trace_compiler` are
    /// [`crate::jit::NullJitBackend`]. See [`crate::vm::jit_state`].
    ///
    /// `#[doc(hidden)] pub` so the `luna` crate's
    /// `extern "C"` JIT helpers can write `vm.jit.pending_err`
    /// directly (same pattern as the pre-A2 `pub Vm::jit_pending_err`
    /// field). Not part of the embedder-facing API surface.
    #[doc(hidden)]
    pub jit: crate::vm::jit_state::JitState,

    /// B12 host roots — append-only `Vec<Value>` traced as an extra
    /// GC root set. `Lua` facade handles (`LuaFunction`, `LuaTable`,
    /// `LuaRoot`) hold indices into this vector so the underlying
    /// `Gc<T>` stays alive across `eval` calls / yield boundaries.
    ///
    /// v1.1 strategy: append-only with explicit `unpin_all` / new Vm.
    /// Slot recycling lands in Phase 3 alongside B8 LuaUserdata, when
    /// the trade-offs between `Drop` plumbing and append-only memory
    /// growth have a richer ergonomics envelope to live in.
    pub(crate) host_roots: Vec<crate::vm::host_roots::HostRootSlot>,
    /// v1.3 Phase SR — recycled-slot index pool. `pin_host` pops the
    /// back if non-empty, else extends `host_roots`. Generation
    /// overflow at `u32::MAX` retires the slot (NOT pushed here).
    pub(crate) host_roots_free: Vec<u32>,

    /// v1.2 Track B — per-Vm cache of `Gc<Table>` metatables keyed
    /// by `TypeId::of::<T>()` for embedder types implementing
    /// [`crate::vm::userdata_trait::LuaUserdata`]. Populated lazily by
    /// [`Vm::register_userdata`]; metatables are pinned via
    /// [`Vm::pin_host`] at registration time so the entry's
    /// `Gc<Table>` stays live for the rest of the Vm's lifetime.
    pub(crate) userdata_metatables:
        std::collections::HashMap<std::any::TypeId, Gc<crate::runtime::table::Table>>,

    /// B6 — classification of the most recent error raised on this Vm.
    /// Embedders read via [`Vm::error_kind`]; the dispatcher sets it
    /// at well-known sites (syntax errors, instr-budget trips, native
    /// callback errors, type errors).
    pub(crate) last_error_kind: crate::vm::error::LuaErrorKind,

    /// B6 — `(source_name, line)` of the most recent error. Set by the
    /// dispatcher / lexer / parser; cleared when a new call_value
    /// enters cleanly.
    pub(crate) last_error_source: Option<(String, u32)>,

    /// v1.1 B10 Stage 1 — when `true`, `instr_budget` exhaustion in
    /// the dispatcher hot loop yields cooperatively (sets
    /// [`Vm::host_yield_pending`] + returns a sentinel `Err` walked up
    /// to `EvalFuture::poll`) instead of returning a real
    /// "instruction budget exceeded" error. Set by [`Vm::eval_async`]
    /// for the duration of the future; restored to `false` on
    /// `Poll::Ready`. The sync `Vm::eval` / `Vm::call_value` paths
    /// leave it `false` so v1.0 behavior is preserved exactly.
    pub(crate) async_mode: bool,

    /// v1.1 B10 Stage 1 — host waker cloned by `EvalFuture::poll`
    /// before driving a slice. The dispatcher itself does not call it
    /// (the future's poll loop does `wake_by_ref` after observing
    /// `BudgetExhausted`), but storing the waker keeps the door open
    /// for Stage 2 async natives to wake the host directly from a
    /// helper future.
    pub(crate) async_waker: Option<std::task::Waker>,

    /// v1.1 B10 Stage 1 — per-poll opcode quota loaded into
    /// `instr_budget` at the start of each `EvalFuture::poll` slice.
    /// Default 10_000 (RFC §D5). Tunable via
    /// [`Vm::set_async_slice`].
    pub(crate) async_slice_size: i64,

    /// v1.1 B10 Stage 1 — set by the dispatcher when an async-mode
    /// budget exhaustion fires; checked by `exec_with` (so the
    /// sentinel propagates without `unwind` running, mirroring
    /// `yielding.is_some()`) and by `call_value_impl` (so the call
    /// frames survive for the next poll). Cleared by `drive_one`
    /// after translating it to `DispatchOutcome::BudgetExhausted`.
    pub(crate) host_yield_pending: bool,

    /// v1.1 B10 Stage 2 — set by the dispatcher's native-call path
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

    /// v1.1 B10 Stage 2 — companion to `pending_async_native_fut`:
    /// the `(func_slot, nargs, nresults, gc_top)` quad needed to
    /// commit the future's eventual `Ok(nret)` back into the calling
    /// frame's expected result slots. Recorded by the dispatcher;
    /// consumed by [`Vm::commit_async_native_result`] after the
    /// future resolves.
    pub(crate) pending_async_native_ctx: Option<AsyncNativeCallCtx>,
}

/// v1.1 B10 Stage 2 — call-site context an in-flight async native
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
    /// `native_nresults`/`gc_top` bookkeeping; reserved for Stage 3+
    /// hook firing + traceback shaping. Not yet read in Stage 2.
    #[allow(dead_code)]
    pub nargs: u32,
    pub nresults: i32,
    /// Recorded for Stage 3+ traceback + GC-root-window auditing.
    /// Stage 2 reads `Vm.gc_top` directly post-resume, so this is
    /// unread today; carried so an Stage 3 audit can confirm the
    /// pre-suspend root window matches the post-resume one.
    #[allow(dead_code)]
    pub gc_top: u32,
}

/// Per-thread debug hook state (PUC `lua_State` hook/hookmask/basehookcount/
/// hookcount). `func` is the Lua hook; the booleans are the PUC mask bits.
#[derive(Clone, Copy, Default)]
pub struct HookState {
    /// the hook function (`None` when no hook is installed)
    pub func: Option<Value>,
    /// v1.1 B11 — Rust-side debug hook. Fires alongside the Lua hook
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

/// Rust-side debug hook callback (B11). Receives the `Vm` plus a
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

/// A resolved debug stack level: a real Lua frame (by index into `frames`) or a
/// synthetic C frame for a call_value boundary.
pub(crate) enum DbgKind {
    Lua(usize),
    /// a synthetic C level; the index is the `from_c` Lua frame it sits below,
    /// used to name the native via its invoking call instruction.
    C(usize),
    /// PUC `CIST_TAIL` placeholder — a Lua-to-Lua tail call collapsed the
    /// caller's activation, so `debug.getinfo(level)` at this slot returns
    /// `what = "tail"` / `short_src = "(tail call)"` / `linedefined = -1` /
    /// `func = nil` and `getfenv(level)` errors (5.1 db.lua :336/:341 pin
    /// both shapes). The index points at the *tail-called* frame whose
    /// `is_tail` flag induced this synthetic level.
    Tail(#[allow(dead_code)] usize),
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

/// Debug-name spelling for a metamethod event tag (the bare `"index"` /
/// `"gc"` / … stored in `Frame.tm`), as `getinfo("n").name` reports it.
///
/// PUC 5.2/5.3 keep the leading `"__"` for every event; 5.4+ strips it for
/// every event *except* `__gc` (`funcnamefromcall` returns the literal
/// `"__gc"` string for `CIST_FIN`, whereas `funcnamefromcode` does
/// `getstr(tmname[tm]) + 2` to skip the `__`).
fn tm_debug_name(version: LuaVersion, tm: &str) -> String {
    if version <= LuaVersion::Lua53 {
        format!("__{tm}")
    } else if tm == "gc" {
        "__gc".to_string()
    } else {
        tm.to_string()
    }
}

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

/// PUC MAXTAGLOOP: bound on `__index`/`__newindex` chains.
const MAX_TAG_LOOP: u32 = 2000;
/// PUC `MAXCCMT`: bound on a `__call` metamethod chain (lvm.c). 200 chains
/// is more than any reasonable program needs and matches PUC 5.4/5.5; the
/// earlier `15` here was tight enough to fire on calls.lua :194 (N=20).
const MAX_CCMT: u32 = 200;
/// PUC LUAI_MAXCCALLS analogue: native↔Lua nesting bound.
const MAX_C_DEPTH: u32 = 200;
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

impl Drop for Vm {
    fn drop(&mut self) {
        // state close: run `__gc` for every still-registered finalizable before
        // the heap frees them (PUC separatetobefnz(g,1) + callallpending). A
        // single pass — objects created by a closing finalizer are not
        // re-finalized (they go to the heap's free list directly).
        self.heap.queue_all_finalizers();
        self.run_finalizers();
    }
}

// P17-D Week 1 scaffold — split-borrow free fn helpers for frames
// push/pop with shadow counter `frames_top: u32`. Free fns (not Vm
// methods) so callers can pass `&mut self.frames` + `&mut self.frames_top`
// as split borrows, allowing other `&mut self.field` reads inside the
// CallFrame construction (e.g. `std::mem::take(&mut self.pending_tm)`).
//
// Week 1 has NO readers yet; the shadow just stays in sync + asserts.
// Week 2 begins migrating hot-path readers (materialize_frames helper)
// to consume `frames_top` and a flat array in place of the Vec.
#[inline(always)]
fn frames_push_sync(frames: &mut Vec<CallFrame>, frames_top: &mut u32, cf: CallFrame) {
    frames.push(cf);
    // Shadow maintenance is debug-only: release builds skip the
    // increment + assertion entirely. The shadow's purpose in Week 1
    // is to VERIFY the assumed invariant (frames_top == frames.len())
    // across all push/pop sites; once Week 2+ migrates readers to
    // consume the shadow, release will run the increment unconditionally.
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
fn frames_pop_sync(frames: &mut Vec<CallFrame>, frames_top: &mut u32) -> Option<CallFrame> {
    let r = frames.pop();
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

impl Vm {
    /// P17-D Week 1 — re-sync `frames_top` after a bulk `frames: Vec`
    /// swap (take_ctx, put_ctx, load_coro_ctx). Must be called after
    /// the Vec replacement to keep the shadow valid.
    #[inline(always)]
    fn frames_resync(&mut self) {
        // Debug-only Week 1 — see `frames_push_sync` comment.
        #[cfg(debug_assertions)]
        {
            self.frames_top = self.frames.len() as u32;
        }
    }

    // ====================================================================
    // P17-D v2 Phase 2 — stack-inline frame metadata accessors (unused).
    //
    // These methods read/write the LJ_FR2 marker slots at `stack[base-2]`
    // (closure GCRef) and `stack[base-1]` (FrameMarker as i64). Phase 2
    // ships them WITHOUT call-site usage; Phase 3 migrates push/pop
    // sites to consume them. Phase 4 removes Vec<CallFrame>.
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
    #[allow(dead_code)] // Phase 2 — consumer is Phase 3.
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
    /// P17-D v2 Direction E2 — uses E1's [`Value::tag_byte`] fast-path
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
    /// P17-D v2 Direction E2 — uses E1's [`Value::tag_byte`] fast-path
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
            gc_mode: "incremental",
            gc_top: 0,
            gc_pause: 200,
            gc_stepmul: 100,
            gc_stepsize: 13,
            gc_finalizing: false,
            capi_stack: Vec::new(),
            capi_cstr_pin: None,
            warn_state: WarnState::Off,
            warn_buf: Vec::new(),
            warn_log: Vec::new(),
            instr_budget: None,
            bytecode_loading: true,
            puc_bytecode_loading: false,
            registry: None,
            file_mt: None,
            io_input: None,
            io_output: None,
            hook: HookState::default(),
            in_hook: false,
            pending_tailcalls: 0,
            errored_native: None,
            hook_ftransfer: 0,
            hook_ntransfer: 0,
            pending_tm: None,
            pending_is_hook: false,
            error_traceback: None,
            public_call_depth: 0,
            running_natives: Vec::new(),
            running_native_slots: Vec::new(),
            // v1.1 A2 — JIT-specific state factored into `JitState`
            // sidecar. The `luna` crate's `Vm::new_minimal_with_jit` /
            // `install_jit_backend` / `luaL_newstate` swap in
            // `CraneliftBackend` for callers that want JIT acceleration.
            jit: crate::vm::jit_state::JitState::with_null_backend(),
            // v1.1 B12 — host roots ticket pool for the `Lua` facade.
            host_roots: Vec::new(),
            host_roots_free: Vec::new(),
            // v1.2 Track B — LuaUserdata trait sugar's per-Vm
            // metatable cache. Populated lazily by register_userdata.
            userdata_metatables: std::collections::HashMap::new(),
            // v1.1 B6 — error classification metadata. Defaults to
            // Runtime; set at known sites (syntax / budget trip /
            // native error / type error).
            last_error_kind: crate::vm::error::LuaErrorKind::default(),
            last_error_source: None,
            // v1.1 B10 Stage 1 — async embedder fields. Defaults
            // preserve sync behavior bit-for-bit (`async_mode = false`
            // means the budget hot loop errors out exactly as v1.0).
            async_mode: false,
            async_waker: None,
            async_slice_size: 10_000,
            host_yield_pending: false,
            // v1.1 B10 Stage 2 — pending async-native state. Empty by
            // default; populated only by the dispatcher when an
            // async-marked NativeClosure is invoked under async_mode.
            pending_async_native_fut: None,
            pending_async_native_ctx: None,
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

    /// P09 embedding: build a Vm with no standard libraries loaded. Embedders
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

    /// v1.1 A1 Session C — install a caller-supplied JIT backend. The
    /// `luna` crate uses this to swap in its `CraneliftBackend`; tests
    /// or third-party backends pass their own [`crate::jit::IntChunkCompiler`] /
    /// [`crate::jit::TraceCompiler`] implementations. Re-installing on a Vm whose
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
    }

    /// v1.1 A1 Session A — install the no-op JIT backend. `try_compile`
    /// reports "skipped" so every closure stays on the interpreter
    /// path, and the trace recorder's compile attempt always returns
    /// `None`. Intended for tests that want to verify the trait
    /// boundary works in a JIT-free configuration, and for the future
    /// `luna-core` build path that ships without Cranelift.
    ///
    /// Calling this on a Vm whose closures already populated
    /// `Proto.jit: JitProtoState::Compiled` does NOT evict those
    /// cached entries — the dispatcher will still call into them. For
    /// a truly JIT-free run, call this immediately after construction.
    pub fn install_null_jit(&mut self) {
        self.jit.chunk_compiler = Box::new(crate::jit::NullJitBackend);
        self.jit.trace_compiler = Box::new(crate::jit::NullJitBackend);
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
        self.open_package();
        // PUC 5.2 introduced `bit32` and 5.3 retired it (the native bitwise
        // operators replace it on 64-bit integers). Only expose it under 5.2
        // so bitwise.lua's first line (`bit32.band(...)`) resolves without
        // leaking the global into newer dialects.
        if self.version == LuaVersion::Lua52 {
            self.open_bit32();
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
        crate::vm::lib_os_io::open_package(self);
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

    /// Install the shared string metatable (string library, P04).
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

    /// Parse + compile a chunk and close it over the globals table.
    pub fn load(&mut self, src: &[u8], chunkname: &[u8]) -> Result<Gc<LuaClosure>, SyntaxError> {
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
            crate::vm::dump::undump(src, &mut self.heap, self.version, allow_puc).map_err(
                |msg| SyntaxError {
                    line: 0,
                    msg: msg.into_bytes(),
                },
            )?
        } else {
            let ast = parse(src, self.version)?;
            compile_chunk(&ast, self.version, chunkname, &mut self.heap)?
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

    /// Call any callable value from the host (or from natives like pcall).
    pub fn call_value(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        // host-level entry (no enclosing exec): drop any error state from a
        // prior call that propagated uncaught (`error_traceback` would
        // otherwise leak into the next debug.traceback call).
        if self.public_call_depth == 0 {
            self.error_traceback = None;
        }
        self.public_call_depth += 1;
        // P11-S2 — JIT fast path. A host call with no args targeting a Lua
        // chunk whose body fits the S1 int-arith whitelist short-circuits
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

    /// P11-S2 — peek/populate the Proto's JIT cache slot, returning
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
                // P11-S5c / S5d.J — install the active Vm + closure
                // for any Rust helper the JIT'd code may call (e.g.
                // `luna_jit_new_table`, `luna_jit_upval_get`) via
                // cranelift `Linkage::Import`. RAII clear on return.
                // Chunks with no upvalue reads don't touch the closure
                // slot, paying nothing.
                // v1.1 A1 Session A — route through chunk_compiler so
                // the NullJitBackend path stays inert. Raw-ptr arg
                // avoids the &mut self borrow conflict against the
                // shared self.jit.chunk_compiler read.
                let vm_ptr: *mut Vm = self;
                let _jit_vm_guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                let r = unsafe { f() };
                drop(_jit_vm_guard);
                // P11-S5d.E' — a JIT helper may have detected a metatable
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

    /// P11-S2 / S2c — populate the cache slot. Flips `Untried` to either
    /// `Compiled { … }` or `Failed`; idempotent on already-populated
    /// states (call sites guard with a get before invoking).
    ///
    /// S4: consults a thread-local cross-`Vm` cache keyed by a hash of
    /// `proto.code`. Compiled artefacts live in the thread-local
    /// `JITModule` so their mmap pages outlive the `Vm`; subsequent
    /// `Vm`s loading the same source skip the cranelift compile step
    /// entirely.
    fn populate_jit_cache(&mut self, proto: Gc<crate::runtime::function::Proto>) {
        use crate::runtime::function::JitProtoState;
        let version = self.version();
        let pre53 = version <= crate::version::LuaVersion::Lua53;
        // P11-S5d.J — 5.1 and 5.2 have no Int subtype (all numbers
        // are Float). The JIT's `GetUpval` ValueRead path uses this
        // to default-pin upvalue reads to Float without a tag check.
        let float_only = version <= crate::version::LuaVersion::Lua52;
        match self
            .jit
            .chunk_compiler
            .try_compile(proto, pre53, float_only)
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

    /// P11-S2c.B — `Op::Call` JIT fast path. Run inside `begin_call`
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
        // kind. A Float-typed slot accepts Value::Float verbatim and
        // promotes Value::Int(x) via i64 → f64; a Table-typed slot
        // accepts only Value::Table and passes the raw Gc ptr; an
        // Int-typed slot accepts only Value::Int. Any other shape
        // bails to the interpreter so the call's actual dynamics
        // (metamethod dispatch / type-coerce) take over.
        let mut args: [i64; crate::jit::MAX_JIT_ARITY as usize] =
            [0; crate::jit::MAX_JIT_ARITY as usize];
        for i in 0..num_args as usize {
            let v = self.stack[(func_slot + 1) as usize + i];
            let want_float = (arg_float_mask >> i) & 1 == 1;
            let want_table = (arg_table_mask >> i) & 1 == 1;
            args[i] = match (want_table, want_float, v) {
                (true, _, Value::Table(t)) => t.as_ptr() as i64,
                (false, false, Value::Int(x)) => x,
                (false, true, Value::Float(f)) => f.to_bits() as i64,
                (false, true, Value::Int(x)) => (x as f64).to_bits() as i64,
                _ => return false,
            };
        }
        // P11-S5c / S5d.J — Vm + closure pin for helpers; see the
        // matching guard in `try_jit_call`.
        // v1.1 A1 Session A — route through chunk_compiler.
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
        // P11-S5d.E' — see matching path in `try_jit_call`. A helper
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
            return Err(self.rt_err("stack overflow"));
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
            // v1.1 B10 — a `host_yield_pending` cooperative yield is in
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

    // ---- coroutines (P05) ----

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
        let result = self.close_slots(0, death_err);
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
        self.frames_resync(); // P17-D Week 1 — frames now empty.
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
        self.frames_resync(); // P17-D Week 1 — sync shadow to new Vec.
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
        self.frames_resync(); // P17-D Week 1 — sync shadow to coro's frames.
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
            CoroStatus::Dead => return Err(self.rt_err("cannot resume dead coroutine")),
            _ => return Err(self.rt_err("cannot resume non-suspended coroutine")),
        }
        if self.c_depth >= MAX_C_DEPTH {
            return Err(self.rt_err("C stack overflow"));
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
                    // Capture the error-point traceback (set by `unwind` before
                    // popping the failing frames) and prepend a synthetic
                    // top entry for the C native that initiated the error
                    // (PUC `[C]: in function '<name>'`) so `debug.traceback(co)`
                    // on the dead coroutine still shows the error site
                    // (db.lua :848 family).
                    if drive.is_err() {
                        let mut tb = self.error_traceback.take().unwrap_or_default();
                        if let Some(nm) = self.errored_native.take() {
                            let mut prefixed: Vec<u8> = Vec::new();
                            prefixed.extend_from_slice(
                                format!("\n\t[C]: in function '{nm}'").as_bytes(),
                            );
                            prefixed.extend(tb);
                            tb = prefixed;
                        }
                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                        unsafe { co.as_mut() }.error_traceback = Some(tb);
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
        if self.hook.line
            && let Some(f) = self.frames.last_mut().and_then(CallFrame::lua_mut)
        {
            f.hook_oldpc = f.pc.saturating_sub(1);
        }
    }

    /// Install a hook on `target` (`None`/current thread → the live VM fields;
    /// another, suspended thread → its saved `Coro` state). PUC `debug.sethook`
    /// with an optional thread argument.
    pub(crate) fn set_hook(&mut self, target: Option<Gc<Coro>>, state: HookState) {
        if self.is_current_thread(target) {
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
        // v1.1 B11 — Rust hook fires first (no Vm reentrancy via call_value;
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
                rh(self, evt);
                self.in_hook = was_in_hook;
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

    fn hook_call(&mut self, from_native: bool, nargs: u32) -> Result<(), LuaError> {
        self.hook_call_with(from_native, nargs, false)
    }

    /// Fire the "return" hook on exit from a function, if armed. ftransfer is
    /// the first result slot relative to the activation's func slot, ntransfer
    /// the number of results.
    fn hook_return(
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
        tm: &'static str,
    ) -> Result<(), LuaError> {
        let saved_top = self.top;
        let cont_slot = self.stack.len() as u32;
        self.stack.push(func);
        self.stack.extend_from_slice(args);
        self.top = self.stack.len() as u32;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Meta(MetaCont { action, saved_top }),
                func_slot: cont_slot,
                nresults: 1,
            }),
        );
        let saved_tm = self.pending_tm.replace(tm);
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

    /// `R[dst] := t[key]` for a VM read opcode, resolving `__index` yieldably.
    fn op_index(&mut self, t: Value, key: Value, dst: u32) -> Result<(), LuaError> {
        match self.index_step(t, key)? {
            MmOut::Done(v) => self.stack[dst as usize] = v,
            MmOut::Mm { func, recv } => {
                self.begin_meta_call(func, &[recv, key], MetaAction::Store { dst }, "index")?;
            }
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from index_step"),
        }
        Ok(())
    }

    /// `t[key] := v` for a VM write opcode, resolving `__newindex` yieldably.
    fn op_newindex(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        match self.newindex_step(t, key, v)? {
            MmOut::Done(_) => {}
            MmOut::Mm { func, recv } => {
                self.begin_meta_call(func, &[recv, key, v], MetaAction::Discard, "newindex")?;
            }
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from newindex_step"),
        }
        Ok(())
    }

    /// Apply a comparison opcode's outcome: a known boolean drives the
    /// conditional skip directly; a metamethod is called yieldably, its
    /// truthiness driving the skip on return.
    fn op_compare(
        &mut self,
        step: MmOut,
        l: Value,
        r: Value,
        k: bool,
        tm: &'static str,
    ) -> Result<(), LuaError> {
        match step {
            MmOut::Done(v) => self.cond_skip(v.truthy(), k),
            MmOut::Mm { func, .. } => {
                self.begin_meta_call(func, &[l, r], MetaAction::Compare { k, negate: false }, tm)?;
            }
            MmOut::CompareSynth { func } => {
                // ≤5.3 `__le` falls back to `not __lt(r, l)`; the swap and
                // negation are driven through `MetaAction::Compare` so the
                // metamethod call can yield like any other compare.
                self.begin_meta_call(func, &[r, l], MetaAction::Compare { k, negate: true }, "lt")?;
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
            Some(mt) => mt.get(Value::Str(self.mm_names[mm as usize])),
            None => Value::Nil,
        }
    }

    /// PUC 5.1 `get_compTM`: a comparison metamethod (`__eq` / `__lt` / `__le`)
    /// only fires when both operands carry a metatable that exposes the same
    /// implementation. Returns the metamethod to call, or `Nil` when no
    /// compatible match exists. Used to honour events.lua 5.1 :262's rule
    /// that `c == d` (where `d` has no metatable) falls back to raw equality.
    pub(crate) fn get_comp_mm(&self, l: Value, r: Value, mm: Mm) -> Value {
        let mt1 = self.metatable_of(l);
        let Some(mt1) = mt1 else { return Value::Nil };
        let key = Value::Str(self.mm_names[mm as usize]);
        let tm1 = mt1.get(key);
        if tm1.is_nil() {
            return Value::Nil;
        }
        let mt2 = self.metatable_of(r);
        let Some(mt2) = mt2 else { return Value::Nil };
        if mt1.as_ptr() == mt2.as_ptr() {
            return tm1;
        }
        let tm2 = mt2.get(key);
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
        if matches!(v, Value::Table(_) | Value::Userdata(_))
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
        if self.begin_call(func_slot, Some(nargs), -1, from_c)? {
            self.exec()
        } else {
            // native completed inline; results at func_slot..top
            Ok(self.take_results(func_slot))
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

    /// `collectgarbage("param", name [,value])`: read (or set, returning the
    /// previous value of) a pacing parameter. Returns `None` for an unknown
    /// name so the caller can raise PUC's `invalid parameter` error. The
    /// collector is stop-the-world, so these only round-trip for API fidelity.
    pub(crate) fn gc_param(&mut self, name: &[u8], set: Option<i64>) -> Option<i64> {
        let slot = match name {
            b"pause" => &mut self.gc_pause,
            b"stepmul" => &mut self.gc_stepmul,
            b"stepsize" => &mut self.gc_stepsize,
            _ => return None,
        };
        let prev = *slot;
        if let Some(v) = set {
            *slot = v;
        }
        Some(prev)
    }

    /// Interpreter safe-point auto-GC: FULL incremental Propagate + adaptive
    /// paced sweep via `Vm::gc_step`.
    ///
    /// Round 1/2 of this attempt SIGABRT'd under coroutine + finalizer stress
    /// (suspected missed barrier). Round 3 (STW-mark + paced sweep) hung
    /// heavy.lua. With **born-black during Propagate** landed (@92b22b3) the
    /// suspected UAF is structurally closed — born objects no longer become
    /// dead-white at atomic flip — so Propagate is safe to re-enable here.
    ///
    /// Adaptive budget scales with heap size: 100M-object heap (heavy.lua's
    /// `loadrep` stress) gets a 25M-object budget so a cycle completes in
    /// O(SWEEP_DIVISOR) safe-points regardless of size.
    #[inline(always)]
    pub(crate) fn maybe_collect_garbage(&mut self, live_top: u32) {
        if self.gc_finalizing {
            return;
        }
        if !self.heap.gc_due() {
            return;
        }
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
        // root only the running thread's live registers (PUC marks [stack, top)):
        // freed temporaries above `gc_top` are excluded so weak values stranded
        // there are not pinned. Suspended threads (main_ctx, other coroutines)
        // stay whole-rooted below — safe over-rooting, and they are not the
        // thread whose weak-table loop is under test.
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
        // B12 host roots — Lua-facade handles keep their referenced
        // values alive across calls/yields. Trace the whole vector;
        // unused slots (post-`unpin_all`) carry Value::Nil which the
        // GC ignores.
        for slot in &self.host_roots {
            // v1.3 SR — free-list slots carry Value::Nil (GC no-op).
            roots.push(slot.value);
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
        let (roots, extra) = self.gc_roots();
        let freed = self.heap.collect_ex(&roots, &extra);
        self.run_finalizers();
        freed
    }

    /// PUC 5.1 `collectgarbage` re-raised the first error a `__gc` finalizer
    /// threw; gc.lua's "errors during collection" probe relies on it. This
    /// variant runs the same cycle but propagates the captured finalizer
    /// error to the explicit caller.
    pub(crate) fn collect_garbage_propagating(&mut self) -> Result<usize, LuaError> {
        if self.gc_finalizing {
            return Ok(0);
        }
        let (roots, extra) = self.gc_roots();
        let freed = self.heap.collect_ex(&roots, &extra);
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

    /// Arm the cooperative instruction budget (P09 embedding). The run loop
    /// decrements this once per dispatch turn; on zero it raises a catchable
    /// `"instruction budget exceeded"` error and disarms itself so the host
    /// can resume with a fresh budget on the next call. `None` removes the
    /// cap. Pass `Some(n)` before `eval`/`call_value` for the embedder's
    /// short-script semantics.
    pub fn set_instr_budget(&mut self, budget: Option<i64>) {
        self.instr_budget = budget;
    }

    /// Remaining instruction budget (None when unbounded).
    pub fn instr_budget_remaining(&self) -> Option<i64> {
        self.instr_budget
    }

    /// Toggle the cranelift JIT (P11). Default `true`. Sandbox embedders
    /// **must** disable JIT when relying on `instr_budget` — see the
    /// `jit_enabled` field doc for the rationale.
    pub fn set_jit_enabled(&mut self, enabled: bool) {
        self.jit.enabled = enabled;
    }

    /// Current JIT enable state.
    pub fn jit_enabled(&self) -> bool {
        self.jit.enabled
    }

    /// Toggle the trace JIT (P12). Off by default while the sprint
    /// develops. When enabled, hot back-edges are counted on
    /// `Proto.trace_hot_count`; once the counter passes
    /// `TRACE_HOT_THRESHOLD`, the dispatch loop enters recording
    /// mode at the back-edge target. Stays a no-op until S2's
    /// trace lowerer and S3's dispatcher land.
    pub fn set_trace_jit_enabled(&mut self, enabled: bool) {
        self.jit.trace_enabled = enabled;
    }

    /// P16-A — opt-in flag for the self-link cycle catch. See field
    /// docs for the correctness blocker. Default `false`.
    pub fn set_p16_self_link_enabled(&mut self, enabled: bool) {
        self.jit.p16_self_link_enabled = enabled;
    }

    /// Current state of the P16-A self-link cycle catch.
    pub fn p16_self_link_enabled(&self) -> bool {
        self.jit.p16_self_link_enabled
    }

    /// Current trace-JIT enable state.
    pub fn trace_jit_enabled(&self) -> bool {
        self.jit.trace_enabled
    }

    /// Number of traces that have closed cleanly (looped back to the
    /// head PC) since this Vm was constructed. Cumulative; used by
    /// tests + tuning. Will become the dominant signal once S2's
    /// compile + cache lands.
    pub fn trace_closed_count(&self) -> u64 {
        self.jit.counters.closed
    }

    /// Number of traces that have aborted (exceeded MAX_TRACE_LEN or
    /// hit an un-recordable op — the latter lands at S2).
    pub fn trace_aborted_count(&self) -> u64 {
        self.jit.counters.aborted
    }

    /// P13-S13-G v2 — number of compiled traces whose close shape
    /// is `TraceEnd::InlineAbort` (depth>0 boundary). Such traces
    /// pin `dispatchable=false` because the dispatcher can't
    /// resume at a depth>0 PC without the matching CallFrames.
    /// S4-step4b's frame-mat helper could synthesise those, but
    /// the InlineAbort emit path isn't wired up yet — fresh
    /// pickup work for S13-G v2-full.
    pub fn trace_inline_abort_count(&self) -> u64 {
        self.jit.counters.inline_abort
    }

    /// P13-S13-G v2.5 — see `JitCounters::dispatch_off_reasons`.
    pub fn trace_dispatch_off_reasons(&self) -> &[&'static str] {
        &self.jit.counters.dispatch_off_reasons
    }

    /// P13-S13-G v2.6 — see `JitCounters::compile_failed_reasons`.
    pub fn trace_compile_failed_reasons(&self) -> &[&'static str] {
        &self.jit.counters.compile_failed_reasons
    }

    /// P13-S13-H — see `JitCounters::closed_lens`. Returns
    /// `(is_call_triggered, ops_len)` for every trace that closed.
    pub fn trace_closed_lens(&self) -> &[(bool, usize)] {
        &self.jit.counters.closed_lens
    }

    /// P12-S2.C — number of closed traces the lowerer compiled and
    /// parked on `Proto.traces`. Re-records of the same head_pc are
    /// deduped (the second close finds the head_pc already cached
    /// and skips compile), so this never exceeds `trace_closed_count`.
    pub fn trace_compiled_count(&self) -> u64 {
        self.jit.counters.compiled
    }

    /// P12-S2.C — number of closed traces the lowerer rejected
    /// (any of the bail conditions in
    /// `crate::jit::trace::try_compile_trace`).
    pub fn trace_compile_failed_count(&self) -> u64 {
        self.jit.counters.compile_failed
    }

    /// P12-S3 — number of times the dispatcher jumped into a
    /// compiled trace. Bumps on every entry; `trace_deopt_count`
    /// counts the subset where the trace returned with a parked
    /// `jit_pending_err`.
    pub fn trace_dispatched_count(&self) -> u64 {
        self.jit.counters.dispatched
    }

    /// P12-S3 — number of trace entries that came back with
    /// `jit_pending_err` set (typically a metatable shadowed an
    /// index inside a helper, forcing the dispatcher to fall back
    /// to the interpreter without committing the trace's result).
    pub fn trace_deopt_count(&self) -> u64 {
        self.jit.counters.deopt
    }

    /// P15-A v1 — number of times the dispatcher started a side
    /// trace recording (an `exit_hit_counts` slot crossed
    /// [`crate::jit::trace::HOTEXIT_THRESHOLD`] while `active_trace`
    /// was None and trace JIT was enabled). Each unit is exactly one
    /// `start_side_trace` call; the actual compile success counts
    /// under [`Self::trace_compiled_count`] like any other trace.
    /// Probe use: distinguishes the "side-trace pipeline fired"
    /// signal from the "primary back-edge / call-trigger fired"
    /// signal so v0-v3 architectural progress is visible without
    /// reading per-counter histograms.
    pub fn trace_side_trace_started_count(&self) -> u64 {
        self.jit.counters.side_trace_started
    }

    /// P15-A v2-A — number of side-trace recordings that closed,
    /// compiled successfully, AND patched their parent's
    /// `exit_side_trace_ptrs[exit_idx]`. The parent's IR doesn't
    /// dispatch through these ptrs yet (v2-B/C job), but the
    /// counter + ptr write proves the compile + link pipeline is
    /// complete end-to-end.
    pub fn trace_side_trace_compiled_count(&self) -> u64 {
        self.jit.counters.side_trace_compiled
    }

    /// P15-A v2-C-A5-C — number of side traces that compiled
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

    /// P12-S5-A — sum of NewTable sites the pre-emit escape sweep
    /// classified as `crate::jit::trace::EscapeState::Sinkable`
    /// across every successfully compiled trace on this Vm. The
    /// count is post-demotion: sites pre-emit drops back to Escaped
    /// for not meeting v1 sunk-emit criteria are NOT counted.
    /// `trace_sunk_alloc_count` matches one-for-one today (every
    /// surviving Sinkable site goes through sunk emit).
    pub fn trace_sinkable_seen_count(&self) -> u64 {
        self.jit.counters.sinkable_seen
    }

    /// P14-S14-B v1 — see `JitCounters::accum_bufferable_seen`.
    pub fn trace_accum_bufferable_seen_count(&self) -> u64 {
        self.jit.counters.accum_bufferable_seen
    }

    /// P15-prep — total dispatch hits across all known traces,
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

    /// P15-A v0 — surface every side-exit slot whose hit count is
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

    /// P12-S5-B — sum of NewTable sites that actually took the
    /// sunk-emit path across every successfully compiled trace on
    /// this Vm. Each counted site skips its heap `Gc<Table>`
    /// allocation per dispatch; the array part lives as Cranelift
    /// `Variable`s for the duration of the trace.
    pub fn trace_sunk_alloc_count(&self) -> u64 {
        self.jit.counters.sunk_alloc
    }

    /// P12-S5-C — sum of materialise-helper emit sites across every
    /// successfully compiled trace on this Vm. Each unit is a
    /// (site × cmp side-exit) pair whose IR reconstructs a heap
    /// `Gc<Table>` from the virt slots on deopt — proves S5-C
    /// emit is wiring materialise into the right side-exits.
    pub fn trace_materialize_emit_count(&self) -> u64 {
        self.jit.counters.materialize_emit
    }

    /// P12-S7-A diagnostic — total `Op::Closure` ops the trace JIT
    /// lowered to the `luna_jit_op_closure` helper. Each emitted op
    /// replaces a `Heap::new_closure_inline` call on the dispatch
    /// path; the count is static (one per matching op per compiled
    /// trace), summed at compile success.
    pub fn trace_closure_emit_count(&self) -> u64 {
        self.jit.counters.closure_emit
    }

    /// P12-S4-step1 diagnostic — max `inline_depth` ever seen on any
    /// `RecordedOp` pushed by the recorder. Tells tests + tuning
    /// whether a self-recursive function actually walked the depth
    /// tracker past 0. Saturates at `MAX_INLINE_DEPTH`. Persists
    /// across traces and Vm activations; reset only on `Vm::new`.
    pub fn trace_max_depth_seen(&self) -> u8 {
        self.jit.max_depth_seen
    }

    /// P12-S4-step4b — last live Lua frame (the trace head's frame at
    /// dispatch time). The frame-materialization helper reads `.base`
    /// to compute offsets for each inlined frame's window.
    #[doc(hidden)]
    pub fn jit_last_lua_frame(&self) -> Option<Frame> {
        match self.frames.last() {
            Some(CallFrame::Lua(f)) => Some(*f),
            _ => None,
        }
    }

    /// P12-S4-step4b — ensure the value stack covers indices
    /// `[0..need)`. Extends with Nil if shorter. Called by the
    /// frame-materialization helper before pushing an inlined frame
    /// whose register window may exceed the current stack length.
    #[doc(hidden)]
    pub fn jit_ensure_stack(&mut self, need: usize) {
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
    }

    /// P12-S7-C — trace JIT path for `Op::Close A`. Predicts whether
    /// `__close` handlers would run (any active tbc slot ≥ from
    /// holding a non-nil/false Value); if so, parks a deopt sentinel
    /// in `jit_pending_err` and returns 1 (helper-side bool) so the
    /// IR branches to the deopt block. Otherwise performs the safe
    /// part of close — `close_from(from)` to close open upvals +
    /// drop any drained tbc entries ≥ from — and returns 0.
    ///
    /// Returns are i64-shaped so the cranelift import sig stays
    /// trivial (i64 → i64 mapping).
    #[doc(hidden)]
    pub fn jit_op_close(&mut self, start_offset: u32) -> i64 {
        if self.jit.pending_err.is_some() {
            return 1;
        }
        let Some(f) = self.jit_last_lua_frame() else {
            self.jit.pending_err = Some(self.rt_err("JIT op_close: no Lua frame"));
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
            self.jit.pending_err =
                Some(self.rt_err("JIT deopt: Op::Close with active tbc handler"));
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

    /// P12-S7-B — spill the trace's current value for a register to
    /// the underlying `vm.stack[base + slot_offset]`. Required before
    /// an `Op::Closure` whose inner proto has an `in_stack: true`
    /// upval at `slot_offset` — the helper's `find_or_create_upval`
    /// captures a live pointer to `vm.stack[base + slot_offset]`,
    /// which must hold the right value at call time (trace IR's
    /// Variable hasn't yet been written back).
    ///
    /// Parameters arrive as i64 from the IR: `slot_offset` is the
    /// caller-frame register index (`u32` in practice, depth=0
    /// only — S7-B doesn't support depth>0 Closure); `tag` is the
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

    /// P12-S12-B-v2 — trace JIT path for `Op::TForCall A 0 C`.
    /// Mirrors the interp arm (this file ~L5316): copies the
    /// generator/state/control triple from `R[A..=A+2]` to
    /// `R[A+4..=A+6]` (resizing the stack if needed), then enters
    /// the iterator function via `begin_call`. v2 only handles
    /// `Value::Native` iterators (the canonical `ipairs_iter` /
    /// `next` builtins) — a Lua-closure iterator would push a Lua
    /// frame mid-trace, breaking `recording_frame_base`, so we
    /// deopt by parking a `pending_err` and returning `-1`.
    ///
    /// `slot_offset` is the caller-frame register index (=
    /// `inst.a()` decoded from a u32-wide field). `nvars` is
    /// `inst.c() as i32` — the caller's expected return count.
    /// P12-S12-C v1 — refresh only the raw payload of
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

    /// P12-S12-C v1 — trace JIT path for `Op::Concat A B`.
    ///
    /// Mirrors the interp arm (this file ~L5112): `self.top =
    /// base + a + n; concat_run(base + a)`. Result lands at
    /// `vm.stack[base + a]`. Returns `0` on success, `-1` on
    /// deopt (any error from `concat_run` OR detection that the
    /// metamethod path was taken — `concat_run` returns `Ok(())`
    /// after `begin_meta_call` which has pushed a Lua frame the
    /// trace can't safely continue past).
    ///
    /// The frame-push detection uses `pre/post frames.len()` and
    /// unwinds any pushed frames before deopting, so the
    /// dispatcher's existing deopt path sees a clean stack.
    #[doc(hidden)]
    pub fn jit_op_concat(&mut self, slot_offset: u32, n: i32) -> i64 {
        if self.jit.pending_err.is_some() {
            return -1;
        }
        let Some(f) = self.jit_last_lua_frame() else {
            self.jit.pending_err = Some(self.rt_err("JIT Concat: no Lua frame"));
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
            frames_pop_sync(&mut self.frames, &mut self.frames_top);
        }
        if let Err(e) = result {
            self.jit.pending_err = Some(e);
            return -1;
        }
        if post_frames > pre_frames {
            self.jit.pending_err = Some(self.rt_err("JIT Concat: __concat metamethod path"));
            return -1;
        }
        0
    }

    /// P14-S14-B v2 — pop a reusable `Vec<u8>` from the JIT
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

    /// P14-S14-B v2 — return a previously-acquired buffer to the
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

    /// P14-S14-B v2 — append a LuaStr's bytes to the accumulator
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

    /// P14-S14-B v2 — drain the accumulator buffer into a fresh
    /// `LuaStr` via `heap.intern`, returning the raw ptr bits for
    /// the trace to write into the accumulator slot.
    ///
    /// Returns the LuaStr ptr as i64 on success, 0 on overflow
    /// (the v2 hard cap; the trace deopts).
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
        // v2 hard cap at 256KB per RFC Q3.
        if bytes.len() > 256 * 1024 {
            return 0;
        }
        let gc = self.heap.intern(&bytes);
        gc.as_ptr() as i64
    }

    /// P12-S12-B v2/v3/v4 — trace JIT helper for `Op::TForCall A 0 C`.
    ///
    /// v2 base: copy R[A..=A+2] → R[A+4..=A+6] + `begin_call`.
    /// v3: ipairs `inext` fast path at the top — skip begin_call
    ///     when R[A]=Native(ipairs_iter), R[A+1]=Table no-mt,
    ///     R[A+2]=Int.
    /// v4: batched out-ptr writeback — fill ctrl/key/val raws into
    ///     caller-provided buffers + return R[A+4]'s tag byte. Lets
    ///     emit skip 3 separate `luna_jit_stack_load` calls and 1
    ///     `luna_jit_stack_tag` call by reading the buffer via
    ///     cranelift `stack_load` IR instead. Returns -1 on deopt.
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
        if self.jit.pending_err.is_some() {
            return -1;
        }
        let Some(f) = self.jit_last_lua_frame() else {
            self.jit.pending_err = Some(self.rt_err("JIT TForCall: no Lua frame"));
            return -1;
        };
        let abs = f.base + slot_offset;
        let need = (abs + 7) as usize;
        if self.stack.len() < need {
            self.stack.resize(need, Value::Nil);
        }
        // v3 fast path.
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
            // v2 slow path: copy R[A..=A+2] → R[A+4..=A+6], then
            // route through begin_call. Lua-closure iters would push
            // a Lua frame mid-trace → deopt.
            self.stack[(abs + 4) as usize] = self.stack[abs as usize];
            self.stack[(abs + 5) as usize] = self.stack[(abs + 1) as usize];
            self.stack[(abs + 6) as usize] = self.stack[(abs + 2) as usize];
            if !matches!(self.stack[abs as usize], Value::Native(_)) {
                self.jit.pending_err = Some(self.rt_err("JIT TForCall: non-Native iter (v2 only)"));
                return -1;
            }
            if let Err(e) = self.begin_call(abs + 4, Some(2), nvars, false) {
                self.jit.pending_err = Some(e);
                return -1;
            }
        }
        // v4 batched writeback — fill the caller's buffers with the
        // raw bits of R[A+2] / R[A+4] / R[A+5] so the trace IR can
        // reload via cranelift `stack_load` instead of separate
        // `luna_jit_stack_load` helper calls.
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let ctrl_raw = unsafe { self.stack[(abs + 2) as usize].unpack().1.zero };
        let (key_tag, key_rv) = self.stack[(abs + 4) as usize].unpack();
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let key_raw = unsafe { key_rv.zero };
        let val_raw = if (nvars as usize) >= 2 {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { self.stack[(abs + 5) as usize].unpack().1.zero }
        } else {
            0u64
        };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe {
            ctrl_out.write(ctrl_raw as i64);
            key_out.write(key_raw as i64);
            val_out.write(val_raw as i64);
        }
        key_tag as i64
    }

    /// P12-S12-B-v2 — load the raw `i64` payload of
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

    /// P12-S12-B-v2 — read the tag byte of
    /// `vm.stack[base + slot_offset]`. Used by `Op::TForLoop` emit
    /// to dispatch on the iterator's return-key tag at runtime
    /// (`raw::NIL` → loop end exit, `raw::INT` → continue, other →
    /// deopt for v2).
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

    /// P12-S4-step4b — push a Lua frame onto the call stack with
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
    /// translators (Phase LB Wave 2) live in `crate::vm::dump::puc`.
    pub fn set_puc_bytecode_loading(&mut self, enabled: bool) {
        self.puc_bytecode_loading = enabled;
    }

    /// Current PUC bytecode-loading gate state.
    pub fn puc_bytecode_loading(&self) -> bool {
        self.puc_bytecode_loading
    }

    /// Take the error traceback captured at the latest error point and
    /// reset it. Embedders should call this immediately after a failed
    /// `call_value`/`eval`/`call`/etc. — the next public `call_value`
    /// entry clears it. Returns `None` if no error was in flight.
    pub fn take_error_traceback(&mut self) -> Option<String> {
        self.error_traceback
            .take()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// Arm the soft memory cap (P09 embedding). The run loop checks the
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
            Some(nc) => unsafe {
                let upvals = &(*nc.as_ptr()).upvals;
                upvals.get(i).copied().unwrap_or(Value::Nil)
            },
            None => Value::Nil,
        }
    }

    /// Register a table for finalization if its (just-set) metatable carries a
    /// `__gc` metamethod (PUC luaC_checkfinalizer at setmetatable time — adding
    /// `__gc` to the metatable afterwards does not retroactively register).
    pub(crate) fn check_finalizer(&mut self, t: Gc<Table>) {
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
                let saved_tm = self.pending_tm.replace("gc");
                // PUC `GCTM` also sets `CIST_FIN` on the CALLER's ci before
                // pcall, so `getinfo(2).namewhat` inside the finalizer reads
                // "metamethod" (5.3 db.lua :720 wires up exactly this probe).
                // luna mirrors by temporarily tagging the current top Lua
                // frame's `tm` to "__gc" for the duration of the call.
                let caller_tm_idx = self
                    .frames
                    .iter()
                    .rposition(|cf| matches!(cf, CallFrame::Lua(_)));
                let saved_caller_tm = caller_tm_idx.and_then(|i| {
                    if let CallFrame::Lua(fr) = &mut self.frames[i] {
                        let prev = fr.tm;
                        fr.tm = Some("gc");
                        Some(prev)
                    } else {
                        None
                    }
                });
                if let Err(e) = self.call_value(gc, &[obj]) {
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
                if let (Some(i), Some(prev)) = (caller_tm_idx, saved_caller_tm)
                    && let Some(CallFrame::Lua(fr)) = self.frames.get_mut(i)
                {
                    fr.tm = prev; // prev is Option<&'static str>; restore exactly
                }
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
        // resolve __call handlers iteratively (PUC tryfuncTM loop): each handler
        // is inserted before the value so it becomes the first argument, and a
        // chain of `__call` tables resolves down to a real function.
        let mut chain = 0u32;
        loop {
            match self.stack[func_slot as usize] {
                Value::Closure(cl) => {
                    // P11-S2c.B JIT fast path: if the Proto's body fits
                    // the int-arith whitelist, every arg is `Value::Int`,
                    // and the cached arity matches, skip frame setup and
                    // run the cached native fn in-place.
                    if self.try_jit_call_op(cl, func_slot, nargs, nresults) {
                        self.pending_tailcalls = tailcalls;
                        return Ok(false);
                    }
                    self.pending_tailcalls = tailcalls;
                    self.push_frame(cl, func_slot, nargs, nresults, from_c)?;
                    // P12-S4-step0 — trace-on-call trigger. The frame
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
                        // P13-S13-H — relaxed call-trigger:
                        // `c >= THRESHOLD` (was `c == THRESHOLD`) +
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
                        // P13-S13-K — additionally short-circuit on
                        // `proto.trace_gave_up`. The S13-I discard
                        // cap force-compiles a partial trace and
                        // flips this flag; subsequent calls into
                        // this Proto skip the RefCell borrow + Vec
                        // scan entirely.
                        if proto.trace_gave_up.get() {
                            return Ok(true);
                        }
                        let call_already_cached =
                            proto.traces.borrow().iter().any(|t| t.head_pc == 0);
                        if c >= crate::jit::trace::CALL_HOT_THRESHOLD
                            && self.jit.active_trace.is_none()
                            && !call_already_cached
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
                    // v1.1 B10 Stage 2 — async-marked NativeClosure.
                    // Route through the cooperative-yield mechanism
                    // when async_mode is on; reject when called from
                    // a sync `eval`/`call_value` path (would have no
                    // executor to drive the returned future).
                    if nc.is_async {
                        if !self.async_mode {
                            let s = Value::Str(
                                self.heap.intern(b"async native called in sync context"),
                            );
                            self.last_error_kind = crate::vm::error::LuaErrorKind::Runtime;
                            return Err(LuaError(s));
                        }
                        // Same root-up bookkeeping as the sync path:
                        // pin args + result-count expectation so a
                        // collection across the suspend boundary
                        // keeps the arg window live.
                        self.native_nresults = nresults;
                        self.gc_top = func_slot + nargs + 1;
                        // Transmute the stored NativeFn back to its
                        // real AsyncNativeFn shape. Sound because
                        // `set_async_native` / `create_async_native`
                        // installed an AsyncNativeFn through the
                        // identically-sized fn-pointer slot, and the
                        // `is_async` marker bit is what records that
                        // fact.
                        let async_fn: crate::vm::async_drive::AsyncNativeFn =
                            // SAFETY: same-size fn pointers; provenance
                            // preserved through `mem::transmute`. The
                            // `is_async` marker is the only safe-to-call
                            // gate, set exclusively by
                            // `Vm::create_async_native`.
                            unsafe { std::mem::transmute(nc.f) };
                        let vm_ptr: *mut Vm = self;
                        let fut = async_fn(vm_ptr, func_slot, nargs);
                        // Stash the future + post-call context for
                        // `drive_one` to surface to `EvalFuture::poll`.
                        self.pending_async_native_fut = Some(fut);
                        self.pending_async_native_ctx = Some(AsyncNativeCallCtx {
                            func_slot,
                            nargs,
                            nresults,
                            gc_top: self.gc_top,
                        });
                        // Sentinel Err walked up to `drive_one` (same
                        // shape as `host_yield_pending`'s budget yield).
                        // Value::Nil — never seen by user code.
                        return Err(LuaError(Value::Nil));
                    }
                    // pcall/xpcall are yieldable: rather than calling the
                    // protected function through the Rust stack (which cannot be
                    // suspended), push a continuation frame and drive the call
                    // through the interpreter loop (PUC lua_pcallk). A yield
                    // inside it is preserved with the thread's saved frames.
                    use crate::runtime::value::NativeFn;
                    if std::ptr::fn_addr_eq(nc.f, nat_pcall as NativeFn) {
                        return self.begin_pcall(func_slot, nargs, nresults);
                    }
                    if std::ptr::fn_addr_eq(nc.f, nat_xpcall as NativeFn) {
                        return self.begin_xpcall(func_slot, nargs, nresults);
                    }
                    // pairs(t) with a __pairs metamethod calls it yieldably (PUC
                    // luaB_pairs); without one, fall through to the plain native.
                    if std::ptr::fn_addr_eq(nc.f, nat_pairs as NativeFn) && nargs >= 1 {
                        let arg = self.stack[(func_slot + 1) as usize];
                        if !self.get_mm(arg, Mm::Pairs).is_nil() {
                            return self.begin_pairs(func_slot, nresults);
                        }
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
                    self.running_natives.push(nc);
                    self.running_native_slots.push((func_slot, nargs));
                    // PUC luaD_precall fires the "call" hook for C functions too.
                    // A yield inside the native (coroutine.yield) propagates an
                    // Err and the matching "return" hook fires on resume instead.
                    if let Err(e) = self.hook_call(true, nargs) {
                        self.running_natives.pop();
                        self.running_native_slots.pop();
                        return Err(e);
                    }
                    // P09: trap a Rust panic in the native and surface it as
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
                            // Stash the offending native's name BEFORE the
                            // pop so a dying coroutine's traceback snapshot
                            // can prepend `[C]: in function '<name>'`. Use
                            // pushglobalfuncname (PUC walks package.loaded
                            // to qualify); fall back to "?".
                            self.errored_native =
                                Some(self.pushglobalfuncname(nc.f).unwrap_or_else(|| "?".into()));
                            self.running_natives.pop();
                            self.running_native_slots.pop();
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
                        if let Some(slot) = self.running_native_slots.last_mut() {
                            slot.1 = nargs + nret;
                        }
                        let hr = self.hook_return(true, nargs + 1, nret);
                        if let Some(slot) = self.running_native_slots.last_mut() {
                            slot.1 = nargs;
                        }
                        // restore results into the slot finish_results expects
                        for i in 0..nret {
                            self.stack[(func_slot + i) as usize] =
                                self.stack[(res_dst + i) as usize];
                        }
                        self.running_natives.pop();
                        self.running_native_slots.pop();
                        hr?;
                    } else {
                        self.running_natives.pop();
                        self.running_native_slots.pop();
                    }
                    self.finish_results(func_slot, nret, nresults);
                    // the native may have allocated; collect with the results as
                    // the live boundary (PUC checks GC after a call returns).
                    self.maybe_collect_garbage(self.top);
                    return Ok(false);
                }
                v => {
                    let mm = self.get_mm(v, Mm::Call);
                    if mm.is_nil() {
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

    fn push_frame(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
        from_c: bool,
    ) -> Result<(), LuaError> {
        if func_slot + 256 > MAX_LUA_STACK {
            // PUC `stackerror`: a stack overflow that surfaces while the
            // current activation is inside an xpcall message handler is
            // translated by `luaD_seterrorobj` (LUA_ERRERR) to "error in
            // error handling". errors.lua :606 expects the inner pcall(loop)
            // it runs from within `xpcall(loop, msgh)`'s msgh to fail with a
            // message matching "error handling".
            let msg = if self.msgh_depth > 0 {
                "error in error handling"
            } else {
                "stack overflow"
            };
            return Err(self.rt_err(msg));
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
        // wipe the register window beyond the kept parameters (stale values —
        // required for GC-safety and codegen). The varargs below `base` survive.
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
            return Err(crate::vm::builtins::raise_str(
                self,
                "bad argument #1 to 'pcall' (value expected)",
            ));
        }
        if self.pcall_depth >= MAX_C_DEPTH {
            return Err(self.rt_err("C stack overflow"));
        }
        self.pcall_depth += 1;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
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
    fn begin_xpcall(
        &mut self,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Result<bool, LuaError> {
        if nargs < 2 {
            return Err(crate::vm::builtins::raise_str(
                self,
                "bad argument #2 to 'xpcall' (value expected)",
            ));
        }
        if self.pcall_depth >= MAX_C_DEPTH {
            return Err(self.rt_err("C stack overflow"));
        }
        self.pcall_depth += 1;
        // layout: [xpcall@func_slot, f@+1, msgh@+2, a1@+3, ...]. Stash msgh and
        // close its gap so f's args become [f@+1, a1@+2, ...].
        let handler = self.stack[(func_slot + 2) as usize];
        let nfargs = nargs - 2;
        for i in 0..nfargs {
            self.stack[(func_slot + 2 + i) as usize] = self.stack[(func_slot + 3 + i) as usize];
        }
        self.top = func_slot + 2 + nfargs;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
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
    fn begin_pairs(&mut self, func_slot: u32, nresults: i32) -> Result<bool, LuaError> {
        let arg = self.stack[(func_slot + 1) as usize];
        let mm = self.get_mm(arg, Mm::Pairs);
        // layout becomes [mm@func_slot, t@func_slot+1]; call mm(t) wanting 4.
        self.stack[func_slot as usize] = mm;
        self.top = func_slot + 2;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Pairs,
                func_slot,
                nresults,
            }),
        );
        self.begin_call(func_slot, Some(1), 4, true)?;
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

    /// Pad/announce results sitting at func_slot.
    pub(crate) fn finish_results(&mut self, func_slot: u32, nret: u32, wanted: i32) {
        if wanted < 0 {
            self.top = func_slot + nret;
        } else {
            let wanted = wanted as u32;
            let need = (func_slot + wanted) as usize;
            if self.stack.len() < need {
                self.stack.resize(need, Value::Nil);
            }
            for i in nret..wanted {
                self.stack[(func_slot + i) as usize] = Value::Nil;
            }
            self.top = func_slot + wanted;
        }
    }

    /// v1.1 B10 Stage 1 — current Lua call-frame depth (read-only).
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
            // PUC `checkclosemth`: "variable '<name>' got a non-closable value
            // (a <type> value)"; the local's name comes from the running
            // frame's locvars at this pc.
            let tn = v.type_name();
            let f = self.top_frame();
            let reg = slot - f.base;
            let pc = (f.pc as usize).saturating_sub(1);
            let where_ = match crate::vm::objname::getlocalname(&f.closure.proto, reg, pc) {
                Some(n) => format!("variable '{n}'"),
                None => "to-be-closed slot".to_string(),
            };
            return Err(self.rt_err(&format!("{where_} got a non-closable value (a {tn} value)")));
        }
        debug_assert!(self.tbc.last().is_none_or(|&s| s < slot));
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
            let saved_tm = self.pending_tm.replace("close");
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
            let saved_tm = self.pending_tm.replace("close");
            frames_push_sync(
                &mut self.frames,
                &mut self.frames_top,
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
            frames_pop_sync(&mut self.frames, &mut self.frames_top).expect("no frame")
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

    pub(crate) fn type_err(&mut self, what: &str, v: Value) -> LuaError {
        let extra = self.subject_varinfo(v);
        let tn = self.obj_typename(v);
        self.rt_err(&format!("attempt to {what} a {tn} value{extra}"))
    }

    /// Name the offending operand of the current instruction (PUC varinfo) for
    /// a type error, e.g. " (global 'x')". The faulting value `bad` is matched
    /// to the instruction's subject register(s); a native-raised error whose
    /// current instruction doesn't hold `bad` simply yields "".
    fn subject_varinfo(&self, bad: Value) -> String {
        use crate::vm::isa::Op;
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
            Op::Concat => {
                let a = instr.a();
                for r in a..a + instr.b() {
                    cands.push(r);
                }
            }
            _ => {}
        }
        for reg in cands {
            if self.r(f.base, reg).raw_eq(bad) {
                return match crate::vm::objname::getobjname(p, pc - 1, reg) {
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
        self.rt_err(&format!("attempt to call a {tn} value{extra}"))
    }

    /// Name the offending call target. A metamethod dispatch pushes a `Cont`
    /// frame before the call, so the opcode that triggered it lives in the
    /// nearest *Lua* frame — read that instruction: OP_CALL names the function
    /// register, any metamethod-bearing opcode yields "(metamethod 'event')".
    fn call_target_varinfo(&self, bad: Value) -> String {
        use crate::vm::isa::Op;
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
        match instr.op() {
            Op::Call | Op::TailCall => {
                let reg = instr.a();
                if self.r(f.base, reg).raw_eq(bad) {
                    match crate::vm::objname::getobjname(p, pc - 1, reg) {
                        Some((kind, name)) => format!(" ({kind} '{name}')"),
                        None => String::new(),
                    }
                } else {
                    String::new()
                }
            }
            op => match mm_event_name(op) {
                Some(ev) => format!(" (metamethod '{ev}')"),
                None => String::new(),
            },
        }
    }

    /// "number has no integer representation", enriched (PUC luaG_tointerror)
    /// with a "(field 'x')"-style suffix naming the offending operand of the
    /// current arithmetic instruction when it can be recovered from bytecode.
    fn no_int_rep_err(&mut self) -> LuaError {
        let extra = self.bad_operand_varinfo();
        self.rt_err(&format!("number{extra} has no integer representation"))
    }

    /// Inspect the current frame's faulting instruction: find the register
    /// operand holding a float with no integer representation and name it.
    fn bad_operand_varinfo(&self) -> String {
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
        if !instr.k() {
            regs.push(instr.c());
        }
        for reg in regs {
            let v = self.r(f.base, reg);
            if matches!(v, Value::Float(x) if crate::runtime::value::f2i_exact(x).is_none()) {
                return match crate::vm::objname::getobjname(p, pc - 1, reg) {
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
        let f = self.frames.last().and_then(CallFrame::lua)?;
        let proto = f.closure.proto;
        if proto.source.as_bytes().is_empty() {
            return Some(self.stripped_prefix());
        }
        if proto.lines.is_empty() {
            return None;
        }
        let line = proto.lines[(f.pc as usize).saturating_sub(1).min(proto.lines.len() - 1)];
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let raw = unsafe { crate::runtime::string::bytes_of(proto.source.as_ptr()) };
        let display = crate::vm::lib_debug::chunk_id(raw);
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

    /// Position prefix of the Lua frame `level` steps up from the running C
    /// function (PUC `luaL_where(L, level)`): `level == 1` is the immediate
    /// Lua caller (skipping Cont/C-boundary frames the way `dbg_frame` does),
    /// `level == 2` its caller, and so on. Used by `error(msg, level)` so the
    /// caller's frame is reported even across pcall/xpcall continuations.
    pub(crate) fn position_prefix_at_level(&self, level: i64) -> Option<String> {
        let fi = match self.dbg_frame(level)? {
            DbgKind::Lua(fi) => fi,
            DbgKind::C(_) | DbgKind::Tail(_) => return None,
        };
        let f = self.frames[fi].lua()?;
        let proto = f.closure.proto;
        // PUC luaG_addinfo: a stripped chunk has no source — see
        // `stripped_prefix` for the per-version wording (5.5 vs ≤5.4).
        if proto.source.as_bytes().is_empty() {
            return Some(self.stripped_prefix());
        }
        // a stripped chunk carries no per-instruction line info
        if proto.lines.is_empty() {
            return None;
        }
        let line = proto.lines[(f.pc as usize).saturating_sub(1).min(proto.lines.len() - 1)];
        // PUC `luaG_addinfo` renders source via `luaO_chunkid` (LUA_IDSIZE=60),
        // not the raw chunk name — handles `@file`/`=name` sigils + truncation.
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let raw = unsafe { crate::runtime::string::bytes_of(proto.source.as_ptr()) };
        let display = crate::vm::lib_debug::chunk_id(raw);
        let src = String::from_utf8_lossy(&display).into_owned();
        Some(format!("{src}:{line}: "))
    }

    // ---- the interpreter ----

    fn exec(&mut self) -> Result<Vec<Value>, LuaError> {
        let entry_depth = self.frames.len();
        self.exec_with(entry_depth)
    }

    /// Run from the current top frame down to (but not past) `entry_depth`
    /// frames. Coroutine driving passes `entry_depth = 1` so the whole thread
    /// runs to completion or a yield.
    /// v1.1 B10 Stage 1 — resume the dispatcher from the saved
    /// `entry_depth` (captured pre-yield by `drive_one`). Called by
    /// `EvalFuture::poll` on every poll after the first to walk the
    /// existing call frames until the next `BudgetExhausted` or
    /// terminal `Ok`/`Err`. Not a public-API surface in Stage 1; the
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
                // v1.1 B10 — `host_yield_pending` is the async-mode
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
        // PUC 5.5 `luaG_errormsg` substitutes "<no error object>" when the
        // error object is nil — so `pcall(function() error(nil) end)` returns
        // that string instead of nil, and `assert(nil, nil)` (whose path
        // throws nil via `lua_settop(L, 1)`) also surfaces a string. Earlier
        // dialects (5.4 and below) keep the nil — 5.4 errors.lua :49 asserts
        // `doit("error()") == nil` and luna would fail that if it always
        // substituted. luna's native `error()` still does its own conversion
        // for direct callers.
        if matches!(err, Value::Nil) && self.version >= crate::version::LuaVersion::Lua55 {
            err = Value::Str(self.heap.intern(b"<no error object>"));
        }
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
        // Snapshot the traceback at the error point — before any frame is
        // popped — so an `xpcall` msgh (which runs after the failed frames are
        // gone) can still describe the error site. The handler frame about to
        // be popped (e.g. a `__close` handler with `tm = Some("close")`) is
        // visible here; once popped, `debug.traceback` would miss it.
        // PUC instead runs msgh with the failed stack intact (luaG_errormsg);
        // but doing so when the stack is near `MAX_LUA_STACK` (true overflow
        // recovery — locals.lua:659) re-overflows. Capture-once propagates
        // through nested unwinds (inner→outer) without re-running msgh.
        if self.error_traceback.is_none() {
            self.error_traceback = Some(self.traceback_bytes(1));
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
                    frames_pop_sync(&mut self.frames, &mut self.frames_top);
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
                    frames_pop_sync(&mut self.frames, &mut self.frames_top);
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
                    frames_pop_sync(&mut self.frames, &mut self.frames_top);
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
                            err = e.0;
                            continue;
                        }
                    }
                }
                CallFrame::Cont(nc) => {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top);
                    self.pcall_depth -= 1;
                    let result = match nc.kind {
                        ContKind::Pcall => err,
                        ContKind::Xpcall { handler } => {
                            // PUC keeps `L->errfunc` set across the handler's
                            // call: `luaG_errormsg` re-fires the handler when
                            // it raises (so `xpcall(error, err, 170)` lets the
                            // chain bottom out at err(0) → "END"). luna mirrors
                            // that by looping until the handler returns or
                            // luna's `iters` cap forces termination.
                            //
                            // The cap models PUC's nCcalls soft window
                            // (MAXCCALLS/10*11): once tripped, `stackerror`
                            // raises "C stack overflow" via `luaG_runerror`
                            // which itself re-enters `luaG_errormsg`, so the
                            // handler runs once more with that string and
                            // naturally returns it (errors.lua :637 at N=300).
                            // We count iterations per Cont::Xpcall rather than
                            // a global counter — nested xpcalls each get their
                            // own budget, matching the way PUC's stack frames
                            // accumulate per dispatch path.
                            const MSGH_CAP: u32 = MAX_C_DEPTH;
                            let mut cur_err = err;
                            let mut iters: u32 = 0;
                            let mut capped = false;
                            loop {
                                if iters >= MSGH_CAP && !capped {
                                    cur_err = Value::Str(self.heap.intern(b"C stack overflow"));
                                    capped = true;
                                }
                                iters += 1;
                                self.msgh_depth += 1;
                                let r = self.call_value(handler, &[cur_err]);
                                self.msgh_depth -= 1;
                                match r {
                                    Ok(hr) => {
                                        break hr.first().copied().unwrap_or(Value::Nil);
                                    }
                                    Err(_) if capped => {
                                        // the handler still errored on the
                                        // synthesized "C stack overflow"; fall
                                        // back to PUC's LUA_ERRERR string.
                                        break Value::Str(
                                            self.heap.intern(b"error in error handling"),
                                        );
                                    }
                                    Err(e) => {
                                        cur_err = e.0;
                                    }
                                }
                            }
                        }
                        ContKind::Meta(_) | ContKind::Pairs | ContKind::Close(_) => {
                            unreachable!("Meta/Pairs/Close cont handled above")
                        }
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
                    frames_pop_sync(&mut self.frames, &mut self.frames_top);
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
                            err = e.0;
                            continue;
                        }
                    }
                }
            }
        }
        Unwound::Propagated(LuaError(err))
    }

    fn run(&mut self, entry_depth: usize) -> Result<Vec<Value>, LuaError> {
        loop {
            // Fast-path slow-check gate: most embedders run with both
            // `instr_budget` and `mem_cap` as None, so a single combined
            // is_some test lets the hot loop skip both branches with one
            // load + branch instead of two.
            if self.instr_budget.is_some() || self.heap.mem_cap.is_some() {
                if let Some(b) = self.instr_budget.as_mut() {
                    *b -= 1;
                    if *b <= 0 {
                        self.instr_budget = None;
                        // v1.1 B10 Stage 1 — async-mode cooperative
                        // yield. Set a sentinel flag so `exec_with`
                        // propagates the Err without `unwind` running
                        // (mirroring the `yielding.is_some()` path),
                        // and `call_value_impl` preserves the call
                        // frames for the next `poll`. Translation back
                        // to `DispatchOutcome::BudgetExhausted` happens
                        // in `drive_one`. The Err value itself is
                        // `Value::Nil` — a pure sentinel, never seen by
                        // user code.
                        if self.async_mode {
                            self.host_yield_pending = true;
                            return Err(LuaError(Value::Nil));
                        }
                        // B6: classify the trip so embedders can
                        // distinguish budget exhaustion from a
                        // generic Runtime error and retry / give up
                        // accordingly.
                        self.last_error_kind = crate::vm::error::LuaErrorKind::InstrBudget;
                        let s = Value::Str(self.heap.intern(b"instruction budget exceeded"));
                        return Err(LuaError(s));
                    }
                }
                if let Some(cap) = self.heap.mem_cap
                    && self.heap.bytes() > cap
                {
                    // First try a full collect — embedders set tight caps
                    // and the overshoot may be reclaimable (closures kept
                    // by short-lived frames, intermediate strings). Only
                    // disarm + raise if the cap is still breached after
                    // collection. PUC's `LUA_GCEMERGENCY` path matches.
                    // gc_top must include `self.top` so the running frame's
                    // live locals (e.g. a growing table) are not freed.
                    self.gc_top = self.top;
                    self.collect_garbage();
                    if self.heap.bytes() > cap {
                        self.heap.mem_cap = None;
                        let s = Value::Str(self.heap.intern(b"memory cap exceeded"));
                        return Err(LuaError(s));
                    }
                }
            }
            // Single combined frame fetch: continuation arm OR Lua arm. Saves
            // a second `self.frames.last()` slice access vs the prior split
            // form (LLVM doesn't always CSE these across the cont branch).
            // A continuation frame on top means the call it protected just
            // delivered its results — wrap as `true, results…` and hand to
            // the pcall/xpcall caller. The error path is handled by `unwind`;
            // this branch is only reached on success/resume completion.
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            let frame_peek = unsafe { self.frames.last().unwrap_unchecked() };
            if let &CallFrame::Cont(nc) = frame_peek {
                // a yieldable metamethod returned: complete the interrupted
                // instruction (PUC luaV_finishOp) and resume the running frame.
                if let ContKind::Meta(mc) = nc.kind {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top);
                    let result = if self.top > nc.func_slot {
                        self.stack[nc.func_slot as usize]
                    } else {
                        Value::Nil
                    };
                    self.stack.truncate(nc.func_slot as usize);
                    self.top = mc.saved_top;
                    self.finish_meta(mc.action, result)?;
                    continue;
                }
                // a __close handler returned successfully: discard its
                // results, restore `top` to the slot the handler was called
                // at (the surrounding frame's register window above this slot
                // must stay alloc'd — never truncate the underlying stack),
                // then continue the close chain (next slot, or fire
                // AfterClose). When the close ends an entry activation,
                // drive_close hands the results up to exec_with directly.
                if let ContKind::Close(cc) = nc.kind {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top);
                    self.top = nc.func_slot;
                    if let Some(vals) =
                        self.drive_close(cc.from, cc.pending, cc.after, entry_depth)?
                    {
                        return Ok(vals);
                    }
                    continue;
                }
                // __pairs returned: normalize its results to exactly four
                // (iterator, state, control, closing) at pairs's slot, where
                // the metamethod was called, and hand them to pairs's caller.
                if let ContKind::Pairs = nc.kind {
                    frames_pop_sync(&mut self.frames, &mut self.frames_top);
                    let total = 4u32;
                    let need = (nc.func_slot + total) as usize;
                    if self.stack.len() < need {
                        self.stack.resize(need, Value::Nil);
                    }
                    for s in self.top..(nc.func_slot + total) {
                        self.stack[s as usize] = Value::Nil;
                    }
                    self.top = nc.func_slot + total;
                    if self.frames.len() < entry_depth {
                        return Ok(self.take_results(nc.func_slot));
                    }
                    self.finish_results(nc.func_slot, total, nc.nresults);
                    continue;
                }
                frames_pop_sync(&mut self.frames, &mut self.frames_top);
                self.pcall_depth -= 1;
                // f's results sit at nc.func_slot+1.. (f was called one slot
                // above the continuation), so writing `true` at the slot makes
                // `true, results…` already contiguous.
                let nret = self.top - (nc.func_slot + 1);
                self.stack[nc.func_slot as usize] = Value::Bool(true);
                let total = 1 + nret;
                self.top = nc.func_slot + total;
                if self.frames.len() < entry_depth {
                    return Ok(self.take_results(nc.func_slot));
                }
                self.finish_results(nc.func_slot, total, nc.nresults);
                continue;
            }
            // GC runs only at the allocation safe points below (PUC's
            // `luaC_checkGC` sites), each with a precise `gc_top`; the loop head
            // no longer collects, so a stale full-window `gc_top` cannot leak in.
            //
            // Hot-path frame fetch: the Cont arm above continues the loop,
            // so reaching here means `frame_peek` is the Lua frame. Reuse it
            // rather than re-fetching `self.frames.last()`.
            let f = match frame_peek {
                CallFrame::Lua(f) => f,
                _ => unreachable!("Cont frame survived the dispatch loop head"),
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

            // P12-S1.C/D — trace recording append + close detection.
            // Gated on `trace_jit_enabled` + `active_trace.is_some()`
            // so default dispatch keeps a single not-taken branch.
            //
            // - At the head PC with a non-empty record, the trace has
            //   looped back to its start: mark `closed = true` and
            //   take the record (S2 will compile + cache).
            // - Otherwise, capture the op. If the record overflows
            //   MAX_TRACE_LEN, abort by dropping it.
            if self.jit.trace_enabled
                && let Some(_rec) = self.jit.active_trace.as_mut()
            {
                // P12-S4 — depth tracking. The trace head's frame is
                // at index `recording_frame_base`; every Op::Call that
                // pushes a new frame bumps the live depth, every
                // Op::Return that pops one decrements it.
                //
                // **Three clean-close conditions** (P12-S4-step4a):
                // - `at_head`: cur_depth == 0 AND about-to-execute the
                //   trace's head_pc on its head_proto (loop closed back
                //   to start). Same for loop-triggered and call-triggered
                //   traces — step4a unified the gating so call-triggered
                //   no longer closes on the first re-entry (that left
                //   fib's body at 7 depth=0 ops; step4a lets it inline
                //   up to MAX_INLINE_DEPTH levels before any close).
                // - `returned_past_head`: trace head's frame is gone
                //   (callee returned past it, or the call-trigger
                //   started a recording inside a callee that has now
                //   returned). Whatever ops were recorded form the
                //   trace body; the lowerer treats the partial trace
                //   the same as InlineAbort (dispatchable=false until
                //   step4b's frame materialization lands).
                // - `depth_cap_hit`: cur_depth > MAX_INLINE_DEPTH.
                //   Recording any deeper would just bloat the IR; close
                //   with the body we have. Lowerer's existing length
                //   gate + InlineAbort path handles short bodies.
                let returned_past_head = self.frames.len() <= self.jit.recording_frame_base;
                let cur_depth = if returned_past_head {
                    0
                } else {
                    self.frames.len() - 1 - self.jit.recording_frame_base
                };
                let depth_cap_hit = cur_depth > crate::jit::trace::MAX_INLINE_DEPTH as usize;
                let rec = self.jit.active_trace.as_mut().expect("just checked Some");
                let at_head_loop = cur_depth == 0
                    && !rec.ops.is_empty()
                    && !returned_past_head
                    && std::ptr::eq(cl.proto.as_ptr(), rec.head_proto.as_ptr())
                    && pc == rec.head_pc;
                // P16-A — self-link cycle catch (mirrors LuaJIT's
                // `check_call_unroll` at `lj_record.c:1869`). Trips when:
                //   1. We're about to execute the head_pc on head_proto
                //      at depth > 0 (we're re-entering the trace head
                //      from inside an inlined recursion level — UpRec).
                //   2. The count of ancestor frames in the recording
                //      window that share `head_proto` exceeds
                //      [`RECUNROLL_THRESHOLD`] (default 2).
                // For fib(N): head_pc=0, head_proto=fib. After 2 inline
                // recursion levels are captured, the recorder enters
                // the 3rd nested fib frame, sees cur_depth=3 > 2, and
                // trips this catch — closing with `SelfRecKind::UpRec`.
                // The lowerer's `TraceEnd::SelfLink` tail emits the
                // bump-base + branch-to-self loop body.
                //
                // TailRec vs UpRec: LJ distinguishes via
                // `framedepth + retdepth == 0`. luna doesn't track
                // retdepth separately; cur_depth == 0 with a non-empty
                // call chain in tail position is rare (would require
                // explicit Lua TCO). We use cur_depth > 0 as the UpRec
                // condition (fib's case); cur_depth == 0 with positive
                // ancestor count would route to TailRec, but luna's
                // recorder doesn't currently produce that shape because
                // tail-call elision pops the caller frame and we'd
                // hit `at_head_loop` instead.
                let self_link_trip: Option<crate::jit::trace::SelfRecKind> = {
                    if self.jit.p16_self_link_enabled
                        && !returned_past_head
                        && std::ptr::eq(cl.proto.as_ptr(), rec.head_proto.as_ptr())
                        && pc == rec.head_pc
                        && cur_depth > 0
                    {
                        // Count ancestor frames sharing head_proto.
                        // self.frames[recording_frame_base..] currently
                        // includes the just-pushed frame at the top
                        // (the one about to execute head_pc). Ancestors
                        // = the slice excluding the top frame.
                        let head_proto_ptr = rec.head_proto.as_ptr();
                        let last_idx = self.frames.len() - 1;
                        let mut count = 0usize;
                        for i in self.jit.recording_frame_base..last_idx {
                            if let CallFrame::Lua(f) = &self.frames[i]
                                && std::ptr::eq(f.closure.proto.as_ptr(), head_proto_ptr)
                            {
                                count += 1;
                            }
                        }
                        if count > crate::jit::trace::RECUNROLL_THRESHOLD {
                            // cur_depth > 0 → UpRec (fib pattern).
                            // cur_depth == 0 wouldn't reach this arm.
                            Some(crate::jit::trace::SelfRecKind::UpRec)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                if let Some(kind) = self_link_trip {
                    rec.self_link_kind = Some(kind);
                }
                let should_close =
                    at_head_loop || returned_past_head || depth_cap_hit || self_link_trip.is_some();
                if should_close {
                    // P13-S13-H — long-trace bias: a call-triggered
                    // recording that closed with a very short body
                    // (fib base case: `Lt`/`Jmp`/`Return1` = 3 ops,
                    // binary_trees `make(0)`: 4 ops) is pathological.
                    // Compiling + caching it pins `Proto.traces` to a
                    // trace that the length gate will refuse to
                    // dispatch (per `MIN_DISPATCHABLE_TRUNC_BODY_FLOOR
                    // = 40`), AND blocks the back-edge / longer-call
                    // path from re-recording the same head_pc (the
                    // dedup `already_cached` check below short-
                    // circuits). The fix: discard the short call-
                    // triggered recording WITHOUT caching, and bias
                    // the proto's `call_hot_count` back to
                    // `THRESHOLD - HOT_RETRY_WINDOW` so the next
                    // sequence of calls retries the trigger at a
                    // different (hopefully deeper) recursion point.
                    //
                    // Back-edge triggered traces are exempt — a
                    // tight numeric-for loop's body is legitimately
                    // 3 ops (`Add`, ForLoop) and DOES dispatch
                    // usefully when re-entered many times.
                    // P13-S13-H — coverage heuristic to detect
                    // pathologically partial call-triggered traces:
                    // for self-recursive / branchy protos like
                    // `fib` (~17 bytecode ops) or
                    // `binary_trees.make` (~26 ops), the recorder
                    // can fire at a BASE-case entry (`fib(0)` or
                    // `make(0)`) producing a 3–4 op trace that
                    // covers a tiny fraction of the proto's code.
                    // That trace is doomed by the length gate
                    // post-compile AND blocks any longer follow-up
                    // (the dedup `already_cached` check below). The
                    // fix: discard call-triggered closes where
                    // `rec.ops.len() * 2 < head_proto.code.len()`
                    // (less than half the proto's bytecode), so the
                    // back-edge / longer call path can take over.
                    //
                    // Why coverage > raw length:protos with
                    // intrinsically short bodies (closure
                    // factories: `Closure + Return1` = 2 ops,
                    // simple wrappers: `LoadI + Return1` = 2 ops)
                    // record 100% coverage even at length 2 — those
                    // ARE legitimately short and the closure /
                    // sunk-emit lowering paths (S7-A / S9-C) make
                    // them worth compiling. The heuristic admits
                    // them. fib's `[Lt, Jmp, Return1]` (3 of ~17)
                    // and make's `[Lt, Jmp, LoadI, Return1]` (4 of
                    // ~26) get discarded.
                    //
                    // Back-edge triggered traces are unaffected —
                    // a tight numeric-for body legitimately covers
                    // 3 of ~3 proto ops it can dispatch from
                    // (`Add + ForLoop`) and the recorder fires on
                    // the back-edge, not call entry.
                    //
                    // `call_hot_count` is intentionally NOT reset
                    // (an earlier draft tried `THRESHOLD - 32` but
                    // caused active_trace contention with the
                    // outer back-edge trigger — see
                    // setlist_b_zero_with_call_c_zero_sunk_emits).
                    // We give up on dispatching the pathological
                    // shape on the same proto; the back-edge or a
                    // longer call path on a deeper recursion point
                    // can still record + cache a real trace.
                    let proto_code_len = rec.head_proto.code.len();
                    let is_partial_coverage = rec.ops.len() * 2 < proto_code_len;
                    // P13-S13-I — per-Proto discard cap. The S13-H
                    // relaxed trigger condition (`c >= THRESHOLD &&
                    // !already_cached`) means a Proto whose every
                    // recording is partial-coverage will re-fire the
                    // trigger every call indefinitely (1500+ in
                    // `binary_trees`-pattern test). The cap stops
                    // discarding after `MAX_DISCARDS_PER_PROTO` —
                    // the next close falls through to compile (even
                    // if partial), caches the trace, and the
                    // `already_cached` short-circuit kills the
                    // storm. Dispatch may still be refused
                    // post-compile (length gate), but the recorder
                    // stops churning.
                    const MAX_DISCARDS_PER_PROTO: u32 = 5;
                    let prior_discards = rec.head_proto.trace_discard_count.get();
                    let cap_reached = prior_discards >= MAX_DISCARDS_PER_PROTO;
                    // P13-S13-K — flip the `gave_up` flag the
                    // moment cap is reached (BEFORE the close-
                    // dispatching branch below). The trigger gates
                    // short-circuit on this flag, skipping the
                    // RefCell + linear `already_cached` scan on
                    // every subsequent call to this Proto. Useful
                    // for `binary_trees_pattern`-class loads where
                    // a single Proto sees ~20k calls post-cap.
                    if cap_reached
                        && rec.is_call_triggered
                        && is_partial_coverage
                        && !rec.head_proto.trace_gave_up.get()
                    {
                        rec.head_proto.trace_gave_up.set(true);
                    }
                    if rec.is_call_triggered && is_partial_coverage && !cap_reached {
                        // Tally as closed (for visibility) but DROP
                        // without compile/cache. Use the existing
                        // closed-lens accumulator so probes can
                        // observe the discarded shape.
                        // P13-S13-I — bump discard count BEFORE
                        // dropping the recording so the next
                        // close sees the updated counter.
                        rec.head_proto.trace_discard_count.set(prior_discards + 1);
                        self.jit.counters.closed += 1;
                        self.jit
                            .counters
                            .closed_lens
                            .push((rec.is_call_triggered, rec.ops.len()));
                        self.jit.active_trace = None;
                        // Continue with interp loop — don't
                        // fall through to compile path.
                        // The op at `pc` hasn't dispatched yet;
                        // the outer loop iteration handles it.
                    } else {
                        rec.closed = true;
                        // P12-S2.C — detach the closed record, then try
                        // to compile it. Dedup by `head_pc`: a Proto
                        // already carrying a CompiledTrace for this PC
                        // skips recompile (the hot counter caps
                        // re-recording at `u32::MAX / 2` anyway, but
                        // explicit dedup keeps `Proto.traces` short
                        // for the S3 dispatcher's linear scan).
                        //
                        // No `Vm::run` change for failure: we just bump
                        // the failed counter and drop the record. S3
                        // will read `Proto.traces` to decide whether to
                        // dispatch — until then, this is bookkeeping.
                        let head_pc_val = rec.head_pc;
                        let closed_record = self
                            .jit
                            .active_trace
                            .take()
                            .expect("active_trace was Some this branch");
                        self.jit.counters.closed += 1;
                        self.jit
                            .counters
                            .closed_lens
                            .push((closed_record.is_call_triggered, closed_record.ops.len()));
                        // P12-S5-B fix: cache the trace on the
                        // recorder's *head proto*, not the current
                        // closure's proto. For non-recursive
                        // call-triggered traces, close fires after
                        // `Return1` pops the callee frame — `cl` at
                        // that point is the CALLER's closure, while
                        // `closed_record.head_proto` is the CALLEE's
                        // proto (the one we actually want the trace
                        // to be discoverable from on the next call).
                        // Self-recursive fib closed via depth-cap
                        // mid-recursion so `cl.proto == head_proto`
                        // happened to coincide — this fix makes that
                        // accidental coincidence intentional.
                        let head_proto = closed_record.head_proto;
                        let already_cached = head_proto
                            .traces
                            .borrow()
                            .iter()
                            .any(|t| t.head_pc == head_pc_val);
                        if !already_cached {
                            // Internal-loop = true: the trace runs in
                            // a native loop until a cmp side-exits, so
                            // the dispatcher's per-entry marshal cost
                            // amortizes across the whole run of
                            // iterations the loop's recorded direction
                            // stays valid. The lowerer auto-downgrades
                            // to one-shot for cmp-less or Call-truncating
                            // traces.
                            // P15-A v2-C-A6-5 — side traces MUST NOT
                            // internal-loop. The parent's recorded prefix
                            // (ops at PCs < side trace's head_pc) defines
                            // values for registers the child's body reads
                            // without re-writing each iter — e.g. for
                            // s12_step_b, parent's `pc=19 Add R[12] = R[1]
                            // + R[11]` sets R[12], and the child trace
                            // (head_pc=24) re-runs `pc=20 Move R[1] =
                            // R[12]` each iter via its outer ForLoop
                            // internal-loop, ALWAYS reading the stale
                            // entry-time R[12]. The parent's Add never
                            // re-runs during child's loop, so R[1] gets
                            // pinned to one stale value. Force one-shot
                            // for side traces: each parent-exit round-
                            // trips through dispatcher → parent's Add
                            // runs → side trace runs ONE iter → return.
                            let opts = crate::jit::trace::CompileOptions {
                                internal_loop: closed_record.side_trace_parent.is_none(),
                                pre53: self.version() <= LuaVersion::Lua53,
                            };
                            // v1.1 A1 Session A — route through trace_compiler.
                            match self
                                .jit
                                .trace_compiler
                                .try_compile_trace(&closed_record, opts)
                            {
                                Some(mut ct) => {
                                    // P12-S5-A/B/C — tally Sinkable sites
                                    // + actually-sunk-emit sites + materialise
                                    // emit sites before moving `ct` into
                                    // Proto.traces.
                                    self.jit.counters.sinkable_seen +=
                                        ct.sinkable_sites_seen as u64;
                                    self.jit.counters.accum_bufferable_seen +=
                                        ct.accum_bufferable_seen as u64;
                                    self.jit.counters.sunk_alloc += ct.sunk_alloc_seen as u64;
                                    self.jit.counters.materialize_emit +=
                                        ct.materialize_emit_count as u64;
                                    self.jit.counters.closure_emit += ct.closure_seen as u64;
                                    if ct.is_inline_abort_close {
                                        self.jit.counters.inline_abort += 1;
                                    }
                                    if let Some(reason) = ct.dispatch_off_reason {
                                        self.jit.counters.dispatch_off_reasons.push(reason);
                                    }
                                    // P15-A v2-A — side-trace finalisation.
                                    // Pin `dispatchable=false` so the
                                    // primary lookup `traces.find(|t|
                                    // t.head_pc == pc && t.dispatchable)`
                                    // never matches this entry — the
                                    // side trace is meant to be entered
                                    // ONLY through the parent's exit
                                    // indirection (v2-B/C IR), not the
                                    // back-edge / call-trigger paths.
                                    // Then write the entry fn ptr into
                                    // the parent's `exit_side_trace_ptrs`
                                    // slot so v2-B/C IR can read it.
                                    if let Some((parent_proto, parent_head_pc, parent_exit_idx)) =
                                        closed_record.side_trace_parent
                                    {
                                        ct.dispatchable = false;
                                        let entry_ptr = ct.entry as *const () as *const u8;
                                        let _side_trace_head_pc = closed_record.head_pc;
                                        let parent_traces = parent_proto.traces.borrow();
                                        if let Some(parent_ct) = parent_traces
                                            .iter()
                                            .find(|t| t.head_pc == parent_head_pc)
                                        {
                                            // P15-A v2-C-A5-C — shape-match
                                            // gate. Find the parent's per-exit
                                            // tag snapshot at the wired exit
                                            // (inline / tag / global) and
                                            // check the child's entry_tags
                                            // match. If not, leave the cell
                                            // null + skip cache populate so
                                            // the future v2-C-A2 IR's
                                            // `call_indirect` stays inert at
                                            // this exit (the child's
                                            // shape-specialised IR would
                                            // mis-interpret raw bits the
                                            // parent writes to reg_state).
                                            let inline_n = parent_ct.per_exit_inline.len();
                                            let tags_n = parent_ct.per_exit_tags.len();
                                            let parent_exit_tags_slice: &[
                                            crate::jit::trace::ExitTag
                                        ] = if parent_exit_idx < inline_n {
                                            &parent_ct.per_exit_inline
                                                [parent_exit_idx]
                                                .exit_tags
                                        } else if parent_exit_idx
                                            < inline_n + tags_n
                                        {
                                            &parent_ct.per_exit_tags
                                                [parent_exit_idx - inline_n]
                                                .1
                                        } else {
                                            &parent_ct.exit_tags
                                        };
                                            let shape_ok =
                                                crate::jit::trace::exit_tags_match_entry_tags(
                                                    &ct.entry_tags,
                                                    parent_exit_tags_slice,
                                                    &parent_ct.entry_tags,
                                                );
                                            if !shape_ok {
                                                self.jit.counters.side_trace_shape_mismatch += 1;
                                            }
                                            // P15-A v2-C-A4 — write the child's
                                            // entry fn ptr to BOTH the legacy
                                            // v2-A `exit_side_trace_ptrs[idx]`
                                            // cell (kept so v2-A's
                                            // walk_any_side_ptr_non_null tests
                                            // stay green) AND the per-kind cell
                                            // whose heap address the parent's
                                            // IR baked (v2-C-A2). The IR-baked
                                            // cell is what the call_indirect
                                            // gate actually reads. Only write
                                            // when A5-C shape gate passes.
                                            if shape_ok {
                                                if let Some(cell) = parent_ct
                                                    .exit_side_trace_ptrs
                                                    .get(parent_exit_idx)
                                                {
                                                    cell.set(entry_ptr);
                                                }
                                                // Compute (kind, local) for the
                                                // IR-baked cell. Layout follows
                                                // exit_hit_counts: inline first,
                                                // then per_exit_tags, then the
                                                // global tail slot.
                                                let (sent_kind, sent_local) = if parent_exit_idx
                                                    < inline_n
                                                {
                                                    parent_ct.per_exit_inline[parent_exit_idx]
                                                        .side_trace_ptr
                                                        .set(entry_ptr);
                                                    (
                                                        crate::jit::trace::SIDE_SENT_KIND_INLINE,
                                                        parent_exit_idx as u32,
                                                    )
                                                } else if parent_exit_idx < inline_n + tags_n {
                                                    let local = parent_exit_idx - inline_n;
                                                    if let Some(b) =
                                                        parent_ct.tags_side_trace_ptrs.get(local)
                                                    {
                                                        b.set(entry_ptr);
                                                    }
                                                    (
                                                        crate::jit::trace::SIDE_SENT_KIND_TAG,
                                                        local as u32,
                                                    )
                                                } else {
                                                    parent_ct.global_side_trace_ptr.set(entry_ptr);
                                                    (crate::jit::trace::SIDE_SENT_KIND_GLOBAL, 0)
                                                };
                                                self.jit.counters.side_trace_compiled += 1;
                                                // P15-A v2-D-A8 — flip the
                                                // parent's fast-path hint so
                                                // the dispatcher knows to do
                                                // the tentative decode + cell
                                                // check on subsequent
                                                // dispatches. Set once and
                                                // stays true (we never unwire
                                                // a side trace today).
                                                parent_ct.has_any_side_wired.set(true);

                                                // P15-A v2-C-A1/A4 — populate
                                                // the O(1) lookup cache the
                                                // dispatcher consults on
                                                // sentinel-bit-set returns.
                                                // Key is the encoded sentinel
                                                // (same encoding the IR ORs
                                                // into bits 56..=62 of the
                                                // child's i64 return).
                                                let sentinel =
                                                    crate::jit::trace::encode_side_sentinel(
                                                        sent_kind, sent_local,
                                                    );
                                                let predicted_idx = if std::ptr::eq(
                                                    parent_proto.as_ptr(),
                                                    head_proto.as_ptr(),
                                                ) {
                                                    parent_traces.len() as u32
                                                } else {
                                                    head_proto.traces.borrow().len() as u32
                                                };
                                                parent_ct
                                                    .side_trace_cache
                                                    .borrow_mut()
                                                    .insert(sentinel, predicted_idx);
                                            }
                                        }
                                        drop(parent_traces);
                                    }
                                    head_proto.traces.borrow_mut().push(std::rc::Rc::new(ct));
                                    self.jit.counters.compiled += 1;
                                }
                                None => {
                                    self.jit.counters.compile_failed += 1;
                                    self.jit
                                        .counters
                                        .compile_failed_reasons
                                        .push(self.jit.trace_compiler.last_compile_checkpoint());
                                }
                            }
                        }
                    } // P13-S13-H — close the long-trace-bias else branch
                } else {
                    // P12-S4-step1 + step4a — depth-aware push at the
                    // current `cur_depth`. The `depth_cap_hit` /
                    // `returned_past_head` early-exit is handled by
                    // the `should_close` branch above; reaching here
                    // means `cur_depth <= MAX_INLINE_DEPTH` and the
                    // trace head's frame is still live.
                    let depth_u8 = cur_depth as u8;
                    if depth_u8 > self.jit.max_depth_seen {
                        self.jit.max_depth_seen = depth_u8;
                    }
                    // P12-S9-A — fix up a prior `Op::Call C=0` (multi-
                    // return / variable return count). Recorder pushed
                    // it with var_count=None before the call dispatched;
                    // now that the call has returned and we're about to
                    // push the next op, top reflects the actual return
                    // count. Snapshot top - (caller.base + call.a).
                    if let Some(last) = rec.ops.last_mut()
                        && matches!(last.inst.op(), crate::vm::isa::Op::Call)
                        && last.inst.c() == 0
                        && last.var_count.is_none()
                        && let Some(f) = self.frames.last().and_then(CallFrame::lua)
                    {
                        let from = f.base + last.inst.a();
                        if self.top >= from {
                            last.var_count = Some(self.top - from);
                        }
                    }
                    // P12-S9-A/C — for SetList B=0, snapshot the source
                    // count = top - A - 1 (mirrors Lua's `n = top - ra
                    // - 1` from lvm.c OP_SETLIST). Sources are
                    // R[A+1..top), exclusive top. For Call C=0's
                    // var_count (the return count = top - A inclusive),
                    // see the prior-op fix-up above; here we
                    // initialise the current Call op to None and let
                    // the fix-up on the next op's push populate it.
                    let var_count = if matches!(inst.op(), crate::vm::isa::Op::SetList)
                        && inst.b() == 0
                        && let Some(f) = self.frames.last().and_then(CallFrame::lua)
                    {
                        let from = f.base + inst.a();
                        if self.top > from {
                            Some(self.top - from - 1)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let op = crate::jit::trace::RecordedOp {
                        proto: cl.proto,
                        pc,
                        inst,
                        inline_depth: depth_u8,
                        var_count,
                    };
                    if !rec.push(op) {
                        self.jit.active_trace = None;
                        self.jit.counters.aborted += 1;
                    }
                }
            }

            // P12-S3 — trace JIT dispatcher.
            //
            // When the dispatch loop is about to execute the op at
            // `pc` and there's a `numeric_only` CompiledTrace cached
            // for that `head_pc`, marshal the live regs into an
            // i64 buffer, jump into the trace, and resume the
            // interpreter at the returned continuation PC.
            //
            // Skipped (zero overhead) when `trace_jit_enabled` is
            // false; the lookup is a borrow + scan over
            // `cl.proto.traces`, which is a `Vec` whose size is at
            // most one entry per back-edge per Proto in practice.
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
            // P17-A1 (Path C #3) — single Rc<CompiledTrace> clone instead
            // of 6 per-field Rc clones. proto.traces is now
            // Vec<Rc<CompiledTrace>>; the dispatcher clones ONE Rc and
            // reads fields via auto-deref. fib_28 saves ~5 Rc::clone
            // operations per dispatch × 434k = ~2.2M Rc atomic ops
            // (~1-2% gain measured separately).
            if self.jit.trace_enabled
                && let Some(ct) = {
                    let traces = cl.proto.traces.borrow();
                    traces
                        .iter()
                        .find(|t| t.head_pc == pc && t.dispatchable)
                        .cloned()
                }
            {
                // Path C #6 — borrow Rc<[T]> fields as &Rc<[T]> instead
                // of cloning. The outer `ct: Rc<CompiledTrace>` is held
                // across the entire dispatch block so the fields outlive
                // all consumers. Saves 5 Rc::clone per dispatch.
                let entry_fn = ct.entry;
                let head_pc_val = ct.head_pc;
                let window_size = ct.window_size;
                let exit_tags = &ct.exit_tags;
                let per_exit_tags = &ct.per_exit_tags;
                let per_exit_inline = &ct.per_exit_inline;
                let compile_entry_tags = &ct.entry_tags;
                let global_tag_res_kind = ct.global_tag_res_kind;
                let exit_hit_counts = &ct.exit_hit_counts;
                let max_stack = cl.proto.max_stack as usize;
                let window_size_us = window_size as usize;
                let base_us = base as usize;
                // P12-S4-step3a — `reg_state` sized to the trace's
                // `window_size`, which today equals max_stack but
                // S4-step3b will expand for inlined frames.
                // Marshal-in still only writes [0..max_stack); slots
                // [max_stack..window_size) are zero-initialised and
                // filled by the trace's own GetUpval / arith.
                // P13-S13-D — reuse the Vm's amortised buffers
                // instead of allocating fresh Vecs each dispatch.
                // mem::take leaves an empty placeholder we restore
                // at the end of the dispatch block (success +
                // deopt paths both fall through to the restore).
                let mut entry_tags: Vec<u8> = std::mem::take(&mut self.jit.entry_tags_buf);
                entry_tags.clear();
                entry_tags.reserve(max_stack);
                let mut reg_state: Vec<i64> = std::mem::take(&mut self.jit.reg_state_buf);
                reg_state.clear();
                reg_state.resize(window_size_us, 0i64);
                let mut dispatch_ok = true;
                for i in 0..max_stack {
                    let v = self.stack[base_us + i];
                    let (tag, raw) = v.unpack();
                    entry_tags.push(tag);
                    // P12-S12-C v3 — entry tag guard. The trace's IR
                    // is specialised to the compile-time entry tags
                    // (via current_kinds propagation from
                    // from_entry_tag). A runtime tag mismatch means
                    // body ops would mis-interpret raw bits (e.g.
                    // treat a Str pointer as Int payload → garbage).
                    // Skip dispatch on mismatch so interp handles
                    // this entry shape; the trace stays cached for
                    // future entries that match.
                    if i < compile_entry_tags.len() && tag != compile_entry_tags[i] {
                        dispatch_ok = false;
                        break;
                    }
                    match tag {
                        // Int / Float / Table / Nil all marshal
                        // to raw payload cleanly; the trace's IR
                        // treats the 8-byte slot as an i64 (with
                        // f64 ops bitcasting around the boundary).
                        crate::runtime::value::raw::INT
                        | crate::runtime::value::raw::FLOAT
                        | crate::runtime::value::raw::TABLE
                        | crate::runtime::value::raw::CLOSURE
                        // P12-S12-B-v2 — Native iter slots (e.g.
                        // R[A] = ipairs_iter) are present in
                        // generic-for traces; the raw bits are a
                        // valid `*mut NativeClosure` and round-trip
                        // cleanly.
                        | crate::runtime::value::raw::NATIVE
                        // P12-S12-C v1 — Str slots show up in
                        // string-concat traces; raw bits = `*mut
                        // LuaStr` (interned, GC-managed). Round-
                        // trips cleanly as a heap pointer.
                        | crate::runtime::value::raw::STR
                        | crate::runtime::value::raw::NIL => {
                            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                            reg_state[i] = unsafe { raw.zero as i64 };
                        }
                        _ => {
                            dispatch_ok = false;
                            break;
                        }
                    }
                }

                if dispatch_ok {
                    debug_assert_eq!(head_pc_val, pc, "trace cache hit's head_pc != pc");
                    self.jit.pending_err = None;
                    // P12-S4-step4b-C-2 — snapshot the pre-entry frame
                    // count. A cmp@d>0 side-exit calls the materialize
                    // helper which pushes inlined frames onto
                    // `vm.frames`; on deopt those frames must be popped
                    // before falling through to the interpreter, else
                    // the stack grows unboundedly per deopted dispatch.
                    let pre_frames = self.frames.len();
                    let continuation_pc = {
                        // v1.1 A1 Session A — chunk_compiler.enter
                        // (CraneliftBackend delegates to enter_jit;
                        // NullJitBackend returns an inert guard).
                        let vm_ptr: *mut Vm = self;
                        let _guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                        unsafe { entry_fn(reg_state.as_mut_ptr()) }
                    };
                    self.jit.counters.dispatched += 1;

                    if self.jit.pending_err.is_some() {
                        self.jit.pending_err = None;
                        self.jit.counters.deopt += 1;
                        // P12-S4-step4b-C-2 — unwind any helper-pushed
                        // inlined frames before the interpreter resumes.
                        // Don't restore reg_state — the trace's partial
                        // writes are discarded; interp re-executes from
                        // the original `pc`.
                        while self.frames.len() > pre_frames {
                            frames_pop_sync(&mut self.frames, &mut self.frames_top);
                        }
                    } else {
                        // Restore each slot using the trace's
                        // exit-tag analysis (see ExitTag docs).
                        // P12-S4-step4b-C-2 — decode the IR's
                        // side-exit shape. Upper 32 bits = (site_idx
                        // + 1) for inline cmp side-exits, 0 for
                        // legacy clean-tail / non-inline exits.
                        // P15-A v2-C-A0 — decode lives in
                        // `crate::jit::trace::decode_exit_shape` so
                        // v2-C-A3 can reuse it with the SIDE TRACE's
                        // shape inputs when the sentinel bit
                        // (v2-C-A2) is set on `raw_ret`.
                        let raw_ret = continuation_pc as u64;
                        // P15-A v2-C-A3 — side-trace return decode.
                        // Bit 63 of `raw_ret` is the side-trace
                        // marker the parent's IR OR'd in when it
                        // tail-called into a wired child trace.
                        // Bits 56..=62 carry the sentinel code (the
                        // cache key into the parent's
                        // `side_trace_cache`); bits 0..=55 are the
                        // child's own return value (encoded site or
                        // plain cont_pc) which we MUST decode using
                        // the CHILD's per_exit_inline / per_exit_tags
                        // / exit_tags / exit_hit_counts — not the
                        // parent's. The dispatcher snapshot read
                        // above holds the parent's shapes; when bit
                        // 63 is set we re-fetch the child's via the
                        // sentinel-keyed cache.
                        let from_side_trace = (raw_ret >> 63) & 1 == 1;
                        let (
                            decode_inline,
                            decode_tags,
                            decode_exit_tags,
                            decode_hit_counts,
                            decode_body,
                        ) = if from_side_trace {
                            let sentinel_code = ((raw_ret >> 56) & 0x7F) as u32;
                            let body = raw_ret & 0x00FF_FFFF_FFFF_FFFFu64;
                            let traces = cl.proto.traces.borrow();
                            let child_idx = traces
                                .iter()
                                .find(|t| t.head_pc == head_pc_val)
                                .and_then(|pct| {
                                    pct.side_trace_cache.borrow().get(&sentinel_code).copied()
                                });
                            if let Some(idx) = child_idx
                                && let Some(child) = traces.get(idx as usize)
                            {
                                if crate::jit::trace::v2c_probe_enabled() {
                                    eprintln!(
                                        "[v2c-A3-decode] sentinel={:#04x} body={:#018x} child_idx={} child.n_ops={} child.head_pc={} child.window_size={} parent.pc={} parent.window_size={} child.dispatchable={} child.inline_abort={}",
                                        sentinel_code,
                                        body,
                                        idx,
                                        child.n_ops,
                                        child.head_pc,
                                        child.window_size,
                                        pc,
                                        window_size,
                                        child.dispatchable,
                                        child.is_inline_abort_close,
                                    );
                                }
                                (
                                    child.per_exit_inline.clone(),
                                    child.per_exit_tags.clone(),
                                    child.exit_tags.clone(),
                                    child.exit_hit_counts.clone(),
                                    body,
                                )
                            } else {
                                if crate::jit::trace::v2c_probe_enabled() {
                                    eprintln!(
                                        "[v2c-A3-decode] sentinel={:#04x} body={:#018x} child MISS (fallback parent shapes)",
                                        sentinel_code, body,
                                    );
                                }
                                // Cache miss — fall back to parent
                                // shapes with the body bits. Best-
                                // effort; the trace_side_trace_
                                // shape_mismatch_count records this
                                // path indirectly (close-handler
                                // skips wiring on mismatch so we
                                // shouldn't reach here when shape
                                // gate held).
                                (
                                    per_exit_inline.clone(),
                                    per_exit_tags.clone(),
                                    exit_tags.clone(),
                                    exit_hit_counts.clone(),
                                    body,
                                )
                            }
                        } else {
                            // P15-A v2-D — dispatcher-level side-trace
                            // invocation. Replaces v2-C's universal IR
                            // gate (`load + icmp + brif` at every
                            // emit_store_back callsite, which A6/A7
                            // measured as a net perf regression).
                            // A8 fast-path: skip the tentative decode +
                            // child lookup entirely when `has_any_side
                            // _wired == false` (the common case until
                            // the first side trace compiles for this
                            // parent). For fib_10_x10k and other tight
                            // short-trace workloads where most parent
                            // traces never get a wired child, this
                            // collapses the v2-D overhead to a single
                            // `Cell::get()` on the cold path.
                            // A8-revert: A8 had `parent_has_side` short-
                            // circuit + snapshot hoist; mini N=3 showed
                            // A8 lost the btrees_d8 1.02× win (dropped
                            // to 0.95×) WITHOUT helping fib_10 (same
                            // 0.86×). Drop A8 — accept the always-run
                            // v2-D path; the tentative decode + cell
                            // load is cheaper than the cost A8 added.
                            {
                                let tentative = crate::jit::trace::decode_exit_shape(
                                    raw_ret,
                                    per_exit_inline,
                                    per_exit_tags,
                                    exit_tags,
                                );
                                let tentative_exit_idx = tentative.exit_hit_idx;
                                let child_invoke = {
                                    let traces = cl.proto.traces.borrow();
                                    traces.iter().find(|t| t.head_pc == head_pc_val).and_then(
                                        |pct| {
                                            let cell =
                                                pct.exit_side_trace_ptrs.get(tentative_exit_idx)?;
                                            let fn_ptr = cell.get();
                                            if fn_ptr.is_null() {
                                                return None;
                                            }
                                            traces
                                                .iter()
                                                .find(|t| {
                                                    t.entry as *const () as *const u8 == fn_ptr
                                                })
                                                .map(|child| {
                                                    (
                                                        child.entry,
                                                        child.per_exit_inline.clone(),
                                                        child.per_exit_tags.clone(),
                                                        child.exit_tags.clone(),
                                                        child.exit_hit_counts.clone(),
                                                    )
                                                })
                                        },
                                    )
                                };
                                if let Some((cent, cpi, cpt, cet, chc)) = child_invoke {
                                    let child_raw_ret = {
                                        // v1.1 A1 Session A — chunk_compiler.enter
                                        // (side-trace entry).
                                        let vm_ptr: *mut Vm = self;
                                        let _guard =
                                            self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                                        unsafe { cent(reg_state.as_mut_ptr()) }
                                    };
                                    (cpi, cpt, cet, chc, child_raw_ret as u64)
                                } else {
                                    (
                                        per_exit_inline.clone(),
                                        per_exit_tags.clone(),
                                        exit_tags.clone(),
                                        exit_hit_counts.clone(),
                                        raw_ret,
                                    )
                                }
                            }
                        };
                        let decoded = crate::jit::trace::decode_exit_shape(
                            decode_body,
                            &decode_inline,
                            &decode_tags,
                            &decode_exit_tags,
                        );
                        let site_id = decoded.site_id;
                        let cont_pc = decoded.cont_pc;
                        let exit_hit_idx = decoded.exit_hit_idx;
                        let exit_tags_for_pc = decoded.exit_tags_for_pc;
                        // P15-A v2-C-A3 — for side-trace returns
                        // force using_global_exit_tags=false so the
                        // restore loop always takes the per-tag slow
                        // path (the child's global_tag_res_kind
                        // classification isn't plumbed through yet
                        // — TODO for a future polish step).
                        let using_global_exit_tags = if from_side_trace {
                            false
                        } else {
                            decoded.using_global_exit_tags
                        };
                        // P15-prep — increment the counter (saturate
                        // at u32::MAX to avoid wrap on long runs).
                        // P15-A v1 — track whether this increment is
                        // the one that crossed `HOTEXIT_THRESHOLD`
                        // (transition: previous v < threshold, new v
                        // == threshold). The side-trace start is
                        // deferred to just before `continue;` so
                        // vm.stack and frame.pc are fully restored
                        // (the snapshot reads post-restore values).
                        let mut side_trace_should_start = false;
                        // P15-A v2-C-A3 — for side-trace returns the
                        // counter to bump is the CHILD's (decoded
                        // shape lookup) — `exit_hit_idx` is into the
                        // decoded layout, so use the matching
                        // `decode_hit_counts`. For parent decode
                        // they're aliased (clone of the parent's
                        // own Rc).
                        if let Some(c) = decode_hit_counts.get(exit_hit_idx) {
                            let v = c.get();
                            if v < u32::MAX {
                                c.set(v + 1);
                            }
                            if v + 1 == crate::jit::trace::HOTEXIT_THRESHOLD
                                && self.jit.active_trace.is_none()
                                && self.jit.trace_enabled
                            {
                                side_trace_should_start = true;
                            }
                        }
                        // P12-S4-step4b-C-2 — at an inline cmp@d>0
                        // side-exit, the helper has pushed N frames on
                        // top of the trace head's frame and
                        // `exit_tags_for_pc.len()` covers the full
                        // window (caller + each inlined frame's
                        // window). Slots beyond `max_stack` belong to
                        // an inlined frame: their `Untouched` entries
                        // default to Nil (no entry-tag fallback —
                        // marshal-in only captured caller slots) and
                        // we write to interp stack at `base + i` which
                        // mirrors `op_offsets`-derived layout.
                        let slot_count = exit_tags_for_pc.len();
                        // P12-S4-step4b-C-2 — the helper only extends
                        // vm.stack up to the deepest pushed frame's
                        // window, but the exit_tags snapshot covers
                        // the trace's full `window_size` (which
                        // includes depth-N+1 scratch slots that the
                        // trace's IR may have written without a
                        // matching pushed frame). Extend with Nil so
                        // the write at the tail doesn't panic; these
                        // slots get overwritten by the writeback loop
                        // and won't leak meaningful data past the
                        // pushed frames' R[0..max_stack) windows.
                        if self.stack.len() < base_us + slot_count {
                            self.stack
                                .resize(base_us + slot_count, crate::runtime::Value::Nil);
                        }
                        // P13-S13-E — fast-path restore loop. When
                        // we landed on the global `exit_tags`,
                        // dispatch on the compile-time
                        // classification: skip the loop entirely
                        // for `AllUntouched`, do a tag-free
                        // `Value::Int(...)` write per slot for
                        // `AllInt`, otherwise fall through to the
                        // general match-arm loop. site_id > 0
                        // (inline frame mat) and per_exit_tags
                        // hits always take the general path —
                        // their per-side-exit shapes aren't
                        // pre-classified yet.
                        let fast_path_taken = if using_global_exit_tags {
                            match global_tag_res_kind {
                                crate::jit::trace::TagResKind::AllUntouched => {
                                    // No-op: vm.stack already
                                    // matches the trace's post-
                                    // entry state for these
                                    // slots (entry values not
                                    // overridden, or already
                                    // spilled by helpers).
                                    true
                                }
                                crate::jit::trace::TagResKind::AllInt => {
                                    for i in 0..slot_count {
                                        self.stack[base_us + i] =
                                            crate::runtime::Value::Int(reg_state[i]);
                                    }
                                    true
                                }
                                crate::jit::trace::TagResKind::Mixed => false,
                            }
                        } else {
                            false
                        };
                        if !fast_path_taken {
                            for i in 0..slot_count {
                                let tag = match exit_tags_for_pc[i] {
                                    crate::jit::trace::ExitTag::Untouched => {
                                        if i < max_stack {
                                            entry_tags[i]
                                        } else {
                                            crate::runtime::value::raw::NIL
                                        }
                                    }
                                    crate::jit::trace::ExitTag::Int => {
                                        crate::runtime::value::raw::INT
                                    }
                                    crate::jit::trace::ExitTag::Float => {
                                        crate::runtime::value::raw::FLOAT
                                    }
                                    crate::jit::trace::ExitTag::Table => {
                                        crate::runtime::value::raw::TABLE
                                    }
                                    crate::jit::trace::ExitTag::Closure => {
                                        crate::runtime::value::raw::CLOSURE
                                    }
                                    // P12-S6-A1 — trace actively wrote Nil
                                    // to this slot (e.g. via Op::LoadNil).
                                    // Restore as Nil regardless of the entry
                                    // tag, since the i64 payload is 0 and
                                    // packing as the entry tag (e.g. INT)
                                    // would mis-type the slot.
                                    crate::jit::trace::ExitTag::Nil => {
                                        crate::runtime::value::raw::NIL
                                    }
                                    // P12-S12-C v2 — trace wrote a Str ptr
                                    // to this slot (LoadK Str / Move from
                                    // Str / Concat result). Restore as
                                    // Value::Str with raw bits round-
                                    // tripped.
                                    crate::jit::trace::ExitTag::Str => {
                                        crate::runtime::value::raw::STR
                                    }
                                };
                                // SAFETY: tag is from a verified slot
                                // (entry validated above) or pinned by
                                // the exit-tag analysis to INT/TABLE.
                                // The raw payload sits in reg_state[i].
                                // Stack was extended by the materialize
                                // helper for inline frames.
                                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                                self.stack[base_us + i] = unsafe {
                                    Value::pack(
                                        tag,
                                        crate::runtime::value::RawVal {
                                            zero: reg_state[i] as u64,
                                        },
                                    )
                                };
                            }
                        }
                        // P12-S4-step4b-C-2 — for non-inline exits the
                        // helper was never called (no metas chain for
                        // this cont_pc), so `frames.last()` is the
                        // trace head's frame and we set its pc to
                        // cont_pc as before. For inline exits the
                        // helper baked the side-exit PC into the
                        // innermost frame's `pc` at push time
                        // (chain.last().pc was overridden at emit),
                        // so this assignment to `frames.last_mut().pc
                        // = cont_pc` is a redundant-but-correct
                        // confirmation.
                        let _ = &per_exit_inline; // hold the Rc alive across dispatch
                        // P12-S4-step4b-C-2 — for inline side-exits the
                        // helper has pushed N frames on top. The trace
                        // head frame is at `pre_frames - 1`; set its
                        // pc to `head_resume_pc` so when the chain
                        // eventually pops back to it, interp resumes
                        // PAST the trace's depth-0 Op::Call instead of
                        // restarting from `head_pc` and re-triggering
                        // dispatch (infinite loop). The innermost
                        // (helper-pushed) frame already has its pc
                        // baked in at compile time, but we still
                        // assign `cont_pc` below for parity with the
                        // non-inline path (no-op).
                        if site_id > 0 {
                            let idx = (site_id - 1) as usize;
                            let head_resume_pc = decode_inline[idx].head_resume_pc;
                            if pre_frames > 0 {
                                if let CallFrame::Lua(f) = &mut self.frames[pre_frames - 1] {
                                    f.pc = head_resume_pc;
                                }
                            }
                        }
                        let frames_len_now = self.frames.len();
                        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                        match unsafe { self.frames.last_mut().unwrap_unchecked() } {
                            CallFrame::Lua(fmut) => {
                                if crate::jit::trace::v2c_probe_enabled() {
                                    eprintln!(
                                        "[v2c-set-pc] from_side={} sentinel_or_raw={:#018x} prev_pc={} new_cont_pc={} site_id={} frames.len={} pre_frames={} max_stack={}",
                                        from_side_trace,
                                        raw_ret,
                                        fmut.pc,
                                        cont_pc,
                                        site_id,
                                        frames_len_now,
                                        pre_frames,
                                        max_stack,
                                    );
                                }
                                fmut.pc = cont_pc;
                            }
                            _ => unreachable!("Cont frame at trace dispatch"),
                        }
                        // P15-A v1 — deferred side-trace start. The
                        // increment block above flagged this exit's
                        // hit count crossing HOTEXIT_THRESHOLD; now
                        // that vm.stack is restored and frame.pc is
                        // settled, snapshot entry_tags from the
                        // resume frame's window and create the
                        // recorder. The recorder's first push fires
                        // on the next interp iteration at cont_pc.
                        //
                        // `head_proto` for the side trace = cl.proto
                        // (trace JIT only inlines self-recursive
                        // calls today, so cont_pc always lands in
                        // the same proto as the parent). Frame base
                        // is the resume frame (top of `self.frames`
                        // — inline-pushed frames moved this).
                        if side_trace_should_start {
                            let (resume_base, resume_proto) = match self.frames.last() {
                                Some(CallFrame::Lua(f)) => (f.base as usize, f.closure.proto),
                                _ => (base_us, cl.proto),
                            };
                            let resume_max_stack = resume_proto.max_stack as usize;
                            let mut side_entry_tags: Vec<u8> = Vec::with_capacity(resume_max_stack);
                            // Extend stack if cont_pc's frame window
                            // overhangs the current stack len (rare,
                            // but inline-pushed frame stack writes
                            // only covered the trace's writeback).
                            if self.stack.len() < resume_base + resume_max_stack {
                                self.stack.resize(
                                    resume_base + resume_max_stack,
                                    crate::runtime::Value::Nil,
                                );
                            }
                            for i in 0..resume_max_stack {
                                let (tag, _) = self.stack[resume_base + i].unpack();
                                side_entry_tags.push(tag);
                            }
                            self.jit.active_trace =
                                Some(Box::new(crate::jit::trace::TraceRecord::start_side_trace(
                                    resume_proto,
                                    cont_pc,
                                    side_entry_tags,
                                    cl.proto,
                                    head_pc_val,
                                    exit_hit_idx,
                                )));
                            self.jit.recording_frame_base = self.frames.len() - 1;
                            self.jit.counters.side_trace_started += 1;
                        }
                        // P13-S13-D — put the dispatch buffers back
                        // before the `continue;` so the next
                        // dispatch picks up the same allocation.
                        self.jit.reg_state_buf = reg_state;
                        self.jit.entry_tags_buf = entry_tags;
                        continue;
                    }
                }
                // P13-S13-D — !dispatch_ok / deopt path / non-cont
                // exit also restore the buffers before falling
                // through to the interp.
                self.jit.reg_state_buf = reg_state;
                self.jit.entry_tags_buf = entry_tags;
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
            match unsafe { self.frames.last_mut().unwrap_unchecked() } {
                CallFrame::Lua(fmut) => fmut.pc = pc + 1,
                _ => unreachable!("Cont frame at pc bump"),
            }

            // count + line hooks (PUC traceexec): before executing the
            // instruction. Skipped while the hook itself runs.
            if self.hook.func.is_some() || self.hook.rust_func.is_some() && !self.in_hook {
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
            }

            match inst.op() {
                Op::Move => {
                    let v = self.r(base, inst.b());
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadI => self.set_r(base, inst.a(), Value::Int(inst.sbx() as i64)),
                Op::LoadF => self.set_r(base, inst.a(), Value::Float(inst.sbx() as f64)),
                Op::LoadK => {
                    let v = cl.proto.consts[inst.bx() as usize];
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadKx => {
                    let extra = cl.proto.code[self.pc_of_top() as usize];
                    self.bump_pc();
                    let v = cl.proto.consts[extra.ax() as usize];
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadFalse => self.set_r(base, inst.a(), Value::Bool(false)),
                Op::LFalseSkip => {
                    self.set_r(base, inst.a(), Value::Bool(false));
                    self.bump_pc();
                }
                Op::LoadTrue => self.set_r(base, inst.a(), Value::Bool(true)),
                Op::LoadNil => {
                    let a = inst.a();
                    for i in 0..=inst.b() {
                        self.set_r(base, a + i, Value::Nil);
                    }
                }
                Op::GetUpval => {
                    let v = self.upval_get(cl, inst.b());
                    self.set_r(base, inst.a(), v);
                }
                Op::SetUpval => {
                    let v = self.r(base, inst.a());
                    self.upval_set(cl, inst.b(), v);
                }
                Op::GetTabUp => {
                    let t = self.upval_get(cl, inst.b());
                    let key = cl.proto.consts[inst.c() as usize];
                    self.op_index(t, key, base + inst.a())?;
                }
                Op::GetTable => {
                    let t = self.r(base, inst.b());
                    let key = self.r(base, inst.c());
                    self.op_index(t, key, base + inst.a())?;
                }
                Op::GetI => {
                    let t = self.r(base, inst.b());
                    self.op_index(t, Value::Int(inst.c() as i64), base + inst.a())?;
                }
                Op::GetField => {
                    let t = self.r(base, inst.b());
                    let key = cl.proto.consts[inst.c() as usize];
                    // v1.2 D4 A1 — fast path: known-Str const key + no
                    // metatable on the table → skip `op_index` /
                    // `index_step`'s MAX_TAG_LOOP setup and the outer
                    // `Value` match. Falls through to the slow path
                    // unchanged when either invariant breaks (so
                    // `__index` metamethods, non-Table receivers, and
                    // non-Str keys behave exactly as before).
                    if let Value::Table(tb) = t
                        && tb.metatable().is_none()
                        && let Value::Str(s) = key
                    {
                        let v = tb.get_str(s);
                        self.stack[(base + inst.a()) as usize] = v;
                    } else {
                        self.op_index(t, key, base + inst.a())?;
                    }
                }
                Op::SetTabUp => {
                    let t = self.upval_get(cl, inst.a());
                    let key = cl.proto.consts[inst.b() as usize];
                    let v = self.r(base, inst.c());
                    self.op_newindex(t, key, v)?;
                }
                Op::SetTable => {
                    let t = self.r(base, inst.a());
                    let key = self.r(base, inst.b());
                    let v = self.r(base, inst.c());
                    self.op_newindex(t, key, v)?;
                }
                Op::SetI => {
                    let t = self.r(base, inst.a());
                    let v = self.r(base, inst.c());
                    self.op_newindex(t, Value::Int(inst.b() as i64), v)?;
                }
                Op::SetField => {
                    let t = self.r(base, inst.a());
                    let key = cl.proto.consts[inst.b() as usize];
                    let v = self.r(base, inst.c());
                    self.op_newindex(t, key, v)?;
                }
                Op::NewTable => {
                    let t = self.heap.new_table();
                    self.set_r(base, inst.a(), Value::Table(t));
                    self.maybe_collect_garbage(base + inst.a() + 1);
                }
                Op::SetList => {
                    let a = inst.a();
                    let abs_a = base + a;
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
                    let Value::Table(t) = self.r(base, a) else {
                        unreachable!("SETLIST on non-table");
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
                Op::SelfOp => {
                    let o = self.r(base, inst.b());
                    self.set_r(base, inst.a() + 1, o);
                    // PUC OP_SELF's C is a constant index when the k-flag is
                    // set; otherwise it points to a register that holds the
                    // (constant-loaded) key. luna's compiler falls back to the
                    // register form when the constant index exceeds OP_SELF's
                    // 8-bit C field (5.1 big.lua's `a:findfield(...)` against
                    // a table with 250+ string keys, where "findfield" lands
                    // past const #255). The exec must honour the same split.
                    let key = if inst.k() {
                        cl.proto.consts[inst.c() as usize]
                    } else {
                        self.r(base, inst.c())
                    };
                    self.op_index(o, key, base + inst.a())?;
                }
                Op::Add => self.arith_rr(inst, base, ArithOp::Add)?,
                Op::Sub => self.arith_rr(inst, base, ArithOp::Sub)?,
                Op::Mul => self.arith_rr(inst, base, ArithOp::Mul)?,
                Op::Mod => self.arith_rr(inst, base, ArithOp::Mod)?,
                Op::Pow => self.arith_rr(inst, base, ArithOp::Pow)?,
                Op::Div => self.arith_rr(inst, base, ArithOp::Div)?,
                Op::IDiv => self.arith_rr(inst, base, ArithOp::IDiv)?,
                Op::BAnd => self.arith_rr(inst, base, ArithOp::BAnd)?,
                Op::BOr => self.arith_rr(inst, base, ArithOp::BOr)?,
                Op::BXor => self.arith_rr(inst, base, ArithOp::BXor)?,
                Op::Shl => self.arith_rr(inst, base, ArithOp::Shl)?,
                Op::Shr => self.arith_rr(inst, base, ArithOp::Shr)?,
                Op::Unm => {
                    let v = self.r(base, inst.b());
                    match coerce_num(v) {
                        Some(Num::Int(i)) => {
                            self.set_r(base, inst.a(), Value::Int(i.wrapping_neg()))
                        }
                        Some(Num::Float(f)) => self.set_r(base, inst.a(), Value::Float(-f)),
                        None => {
                            let mm = self.get_mm(v, Mm::Unm);
                            if mm.is_nil() {
                                return Err(self.type_err("perform arithmetic on", v));
                            }
                            let dst = base + inst.a();
                            self.begin_meta_call(mm, &[v, v], MetaAction::Store { dst }, "unm")?;
                        }
                    }
                }
                Op::BNot => {
                    let v = self.r(base, inst.b());
                    match coerce_num(v) {
                        Some(n) => {
                            let i = self.int_from_num(n)?;
                            self.set_r(base, inst.a(), Value::Int(!i));
                        }
                        None => {
                            let mm = self.get_mm(v, Mm::BNot);
                            if mm.is_nil() {
                                return Err(self.type_err("perform bitwise operation on", v));
                            }
                            let dst = base + inst.a();
                            self.begin_meta_call(mm, &[v, v], MetaAction::Store { dst }, "bnot")?;
                        }
                    }
                }
                Op::Not => {
                    let v = self.r(base, inst.b());
                    self.set_r(base, inst.a(), Value::Bool(!v.truthy()));
                }
                Op::Len => {
                    let v = self.r(base, inst.b());
                    match self.len_step(v)? {
                        MmOut::Done(r) => self.set_r(base, inst.a(), r),
                        MmOut::Mm { func, recv } => {
                            let dst = base + inst.a();
                            self.begin_meta_call(
                                func,
                                &[recv, recv],
                                MetaAction::Store { dst },
                                "len",
                            )?;
                        }
                        MmOut::CompareSynth { .. } => unreachable!("CompareSynth from len_step"),
                    }
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
                Op::Jmp => {
                    let off = inst.sj();
                    // P12-S1.B — trace JIT back-edge counter. A negative
                    // jump offset is a loop back-edge (the only canonical
                    // backward jumps the compiler emits — `while`, `for`,
                    // `repeat`). Tick the per-Proto counter and, once it
                    // exceeds the threshold, log a stub promotion that
                    // S1.C will turn into actual trace recording. The
                    // whole block is gated on `trace_jit_enabled` so
                    // existing benches see one branch-not-taken and no
                    // counter writes.
                    if self.jit.trace_enabled && off < 0 {
                        let proto = cl.proto;
                        let c = proto.trace_hot_count.get();
                        if c < u32::MAX / 2 {
                            proto.trace_hot_count.set(c + 1);
                        }
                        // P13-S13-H — relaxed back-edge trigger:
                        // `c >= THRESHOLD` (was `c == THRESHOLD`) so
                        // a missed crossing (active_trace busy with
                        // a call-trigger, or the recorder slot
                        // happened to be in use) doesn't permanently
                        // lock this back-edge target out. The
                        // `already_cached` short-circuit prevents
                        // duplicate recordings: once a trace is
                        // cached for this target, subsequent
                        // crossings skip the start. This pairs with
                        // S13-H's discard-on-partial-coverage close
                        // handling — when a short call-trigger is
                        // discarded, the back-edge can still find an
                        // open slot at the next iteration.
                        let target_pc = (pc as i32 + 1 + off as i32).max(0) as u32;
                        // P13-S13-K — gave-up short-circuit. Skip
                        // the RefCell borrow + scan when the
                        // S13-I cap force-compiled a partial
                        // trace on this Proto.
                        let back_edge_already_cached = if proto.trace_gave_up.get() {
                            true
                        } else {
                            proto.traces.borrow().iter().any(|t| t.head_pc == target_pc)
                        };
                        if c >= crate::jit::trace::TRACE_HOT_THRESHOLD
                            && self.jit.active_trace.is_none()
                            && !back_edge_already_cached
                        {
                            // Back-edge target = pc after `add_pc(off)`,
                            // i.e. current `pc + 1 + off` (the dispatch
                            // loop has already advanced f.pc to pc+1).
                            let target = (pc as i32 + 1 + off as i32).max(0) as u32;
                            // Snapshot per-slot Value tag at trace
                            // entry so the lowerer's kind tracker
                            // knows which arith path to lower
                            // (iadd vs fadd, etc.).
                            let max_stack = cl.proto.max_stack as usize;
                            let base_us = base as usize;
                            let mut entry_tags = Vec::with_capacity(max_stack);
                            for i in 0..max_stack {
                                let (tag, _) = self.stack[base_us + i].unpack();
                                entry_tags.push(tag);
                            }
                            self.jit.active_trace =
                                Some(Box::new(crate::jit::trace::TraceRecord::start(
                                    cl.proto, target, entry_tags, false,
                                )));
                            // P12-S4 — record the frame the trace
                            // started in. `self.frames.len() - 1`
                            // since we're inside the currently-running
                            // Lua frame's dispatch.
                            self.jit.recording_frame_base = self.frames.len() - 1;
                        }
                    }
                    self.add_pc(off);
                }
                Op::Eq => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    if let (Value::Int(a), Value::Int(b)) = (l, r) {
                        if (a == b) != inst.k() {
                            self.bump_pc();
                        }
                    } else {
                        let step = self.eq_step(l, r);
                        self.op_compare(step, l, r, inst.k(), "eq")?;
                    }
                }
                Op::EqK => {
                    let l = self.r(base, inst.a());
                    let r = cl.proto.consts[inst.b() as usize];
                    if let (Value::Int(a), Value::Int(b)) = (l, r) {
                        if (a == b) != inst.k() {
                            self.bump_pc();
                        }
                    } else {
                        let step = self.eq_step(l, r);
                        self.op_compare(step, l, r, inst.k(), "eq")?;
                    }
                }
                Op::Lt => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    // hot path: Int < Int — drops the MmOut + op_compare match
                    if let (Value::Int(a), Value::Int(b)) = (l, r) {
                        if (a < b) != inst.k() {
                            self.bump_pc();
                        }
                    } else {
                        let step = self.less_step(l, r, false)?;
                        self.op_compare(step, l, r, inst.k(), "lt")?;
                    }
                }
                Op::Le => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    if let (Value::Int(a), Value::Int(b)) = (l, r) {
                        if (a <= b) != inst.k() {
                            self.bump_pc();
                        }
                    } else {
                        let step = self.less_step(l, r, true)?;
                        self.op_compare(step, l, r, inst.k(), "le")?;
                    }
                }
                Op::Test => {
                    let cond = self.r(base, inst.a()).truthy();
                    self.cond_skip(cond, inst.k());
                }
                Op::TestSet => {
                    let v = self.r(base, inst.b());
                    if v.truthy() == inst.k() {
                        self.set_r(base, inst.a(), v);
                    } else {
                        self.bump_pc();
                    }
                }
                Op::Call => {
                    let abs = base + inst.a();
                    let nargs = if inst.b() == 0 {
                        None
                    } else {
                        Some(inst.b() - 1)
                    };
                    let wanted = inst.c() as i32 - 1;
                    self.begin_call(abs, nargs, wanted, false)?;
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
                        if mm.is_nil() {
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
                        // PUC `CIST_TAIL`: the new Lua activation inherits
                        // the popped frame's tailcalls count plus one for
                        // this collapse. 5.1 db.lua :372 hammers 30000
                        // recursive tail calls and expects to see the
                        // synthetic tail level for every one of them.
                        self.pending_tailcalls = fr.tailcalls.saturating_add(1);
                        frames_pop_sync(&mut self.frames, &mut self.frames_top);
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
                    if let Some(vals) = self.begin_close(
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
                Op::ForLoop => {
                    // P12 — trace JIT back-edge counter on the
                    // numeric-for back-edge. ForLoop is always at
                    // a back-edge position (when it continues);
                    // for the trace recorder we treat it as the
                    // close-detection equivalent of `Op::Jmp` with
                    // negative offset. Counter only ticks when the
                    // back-edge will actually fire (count > 0 in
                    // the 5.4+ Int form, comparable predicates in
                    // pre-5.3 / Float). The cheap check up front
                    // matches the for_loop helper's branch.
                    if self.jit.trace_enabled {
                        let a = inst.a();
                        let pre53 = self.version() <= LuaVersion::Lua53;
                        let take_back_edge =
                            match (self.r(base, a), self.r(base, a + 1), self.r(base, a + 2)) {
                                (Value::Int(_), Value::Int(count), Value::Int(_)) if !pre53 => {
                                    count > 0
                                }
                                (Value::Int(cur), Value::Int(lim), Value::Int(st)) if pre53 => {
                                    let next = cur.wrapping_add(st);
                                    if st > 0 { next <= lim } else { next >= lim }
                                }
                                (Value::Float(cur), Value::Float(lim), Value::Float(st)) => {
                                    let next = cur + st;
                                    if st > 0.0 { next <= lim } else { next >= lim }
                                }
                                _ => false,
                            };
                        if take_back_edge {
                            let proto = cl.proto;
                            let c = proto.trace_hot_count.get();
                            if c < u32::MAX / 2 {
                                proto.trace_hot_count.set(c + 1);
                            }
                            if c == crate::jit::trace::TRACE_HOT_THRESHOLD
                                && self.jit.active_trace.is_none()
                            {
                                // ForLoop's back-edge target = pc
                                // after `add_pc(-bx)` runs from the
                                // already-bumped f.pc (= pc + 1).
                                // So target = (pc + 1) - bx.
                                let target = (pc as i32 + 1 - inst.bx() as i32).max(0) as u32;
                                let max_stack = cl.proto.max_stack as usize;
                                let base_us = base as usize;
                                let mut entry_tags = Vec::with_capacity(max_stack);
                                for i in 0..max_stack {
                                    let (tag, _) = self.stack[base_us + i].unpack();
                                    entry_tags.push(tag);
                                }
                                self.jit.active_trace =
                                    Some(Box::new(crate::jit::trace::TraceRecord::start(
                                        cl.proto, target, entry_tags, false,
                                    )));
                                // P12-S4 — record the frame the trace
                                // started in. The currently-running
                                // Lua frame is at len() - 1.
                                self.jit.recording_frame_base = self.frames.len() - 1;
                            }
                        }
                    }
                    self.for_loop(inst, base);
                }
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
                Op::TForLoop => {
                    let a = inst.a();
                    let ctrl = self.r(base, a + 4);
                    if !ctrl.is_nil() {
                        // P12-S12-B v1 — trace JIT back-edge counter on
                        // generic-for back-edge. TForLoop sits at the
                        // tail of `for k,v in expr do ... end`; recorder
                        // treats it as the close-detection equivalent of
                        // a negative Op::Jmp. Gate on `take_back_edge`
                        // (= `ctrl != nil`) so empty-iter loops don't
                        // pollute hot_count. v1 only adds the trigger;
                        // whitelist + helper + emit live in v2.
                        if self.jit.trace_enabled {
                            let proto = cl.proto;
                            let c = proto.trace_hot_count.get();
                            if c < u32::MAX / 2 {
                                proto.trace_hot_count.set(c + 1);
                            }
                            if c == crate::jit::trace::TRACE_HOT_THRESHOLD
                                && self.jit.active_trace.is_none()
                            {
                                // TForLoop back-edge target = pc after
                                // `add_pc(-bx)` runs from the already-
                                // bumped f.pc (= pc + 1). So target =
                                // (pc + 1) - bx, normally landing on
                                // body_top (the op right after TForPrep).
                                let target = (pc as i32 + 1 - inst.bx() as i32).max(0) as u32;
                                let max_stack = cl.proto.max_stack as usize;
                                let base_us = base as usize;
                                let mut entry_tags = Vec::with_capacity(max_stack);
                                for i in 0..max_stack {
                                    let (tag, _) = self.stack[base_us + i].unpack();
                                    entry_tags.push(tag);
                                }
                                // P12-S12-B-v5 — snapshot the iter
                                // fn's address if Native, so the
                                // lowerer can specialise ipairs into
                                // inline Table aget IR.
                                let iter_ptr =
                                    if let Value::Native(n) = self.stack[base_us + a as usize] {
                                        Some(n.f as usize)
                                    } else {
                                        None
                                    };
                                // P12-S12-C v3 — snapshot R[A+5]'s
                                // tag (= current iter's val from
                                // the just-fired TForCall). The v5
                                // inline aget fast_blk emits a
                                // runtime guard against this tag;
                                // mixed-tag arrays deopt rather
                                // than producing garbage pointers
                                // through the v2 spill path.
                                let val_slot = base_us + (a as usize) + 5;
                                let val_tag = if val_slot < self.stack.len() {
                                    Some(self.stack[val_slot].unpack().0)
                                } else {
                                    None
                                };
                                let mut rec = crate::jit::trace::TraceRecord::start(
                                    cl.proto, target, entry_tags, false,
                                );
                                rec.tfor_iter_ptr = iter_ptr;
                                rec.tfor_val_tag = val_tag;
                                self.jit.active_trace = Some(Box::new(rec));
                                self.jit.recording_frame_base = self.frames.len() - 1;
                            }
                        }
                        self.set_r(base, a + 2, ctrl);
                        self.add_pc(-(inst.bx() as i32));
                    }
                }
                Op::Closure => {
                    let proto = cl.proto.protos[inst.bx() as usize];
                    let n_ups = proto.upvals.len();
                    // P11-S5d.M — build upvals on the stack for small
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
                Op::VargIdx => {
                    // R[A] := vararg[R[C]] without allocating: integer key in
                    // [1,n] → that vararg, "n" → the count, else nil.
                    let key = self.r(base, inst.c());
                    let n = n_varargs;
                    let v = match key {
                        Value::Int(k) if k >= 1 && (k as u64) <= n as u64 => {
                            self.stack[(func_slot + k as u32) as usize]
                        }
                        Value::Float(f) if f.fract() == 0.0 && f >= 1.0 && f <= n as f64 => {
                            self.stack[(func_slot + f as u32) as usize]
                        }
                        Value::Str(s) if s.as_bytes() == b"n" => Value::Int(n as i64),
                        _ => Value::Nil,
                    };
                    self.set_r(base, inst.a(), v);
                }
                Op::ErrNNil => {
                    let v = self.r(base, inst.a());
                    if !matches!(v, Value::Nil) {
                        let bx = inst.bx();
                        let name = if bx == 0 {
                            "?".to_string()
                        } else {
                            match cl.proto.consts[(bx - 1) as usize] {
                                Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                                _ => "?".to_string(),
                            }
                        };
                        return Err(self.rt_err(&format!("global '{name}' already defined")));
                    }
                }
                Op::ExtraArg => unreachable!("EXTRAARG executed directly"),
            }
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

    /// Length fast path: a string's byte count or a table's raw border when no
    /// `__len` is present (`Done`); otherwise the `__len` metamethod (`Mm`),
    /// called with the operand twice. Errors for a non-table with no `__len`.
    fn len_step(&mut self, v: Value) -> Result<MmOut, LuaError> {
        match v {
            Value::Str(s) => Ok(MmOut::Done(Value::Int(s.len() as i64))),
            Value::Table(t) => {
                let mm = self.get_mm(v, Mm::Len);
                if mm.is_nil() {
                    Ok(MmOut::Done(Value::Int(t.len())))
                } else {
                    Ok(MmOut::Mm { func: mm, recv: v })
                }
            }
            _ => {
                let mm = self.get_mm(v, Mm::Len);
                if mm.is_nil() {
                    Err(self.type_err("get length of", v))
                } else {
                    Ok(MmOut::Mm { func: mm, recv: v })
                }
            }
        }
    }

    /// PUC luaL_len: the length as an integer, erroring if `__len` returned a
    /// value with no integer representation.
    pub(crate) fn checked_len(&mut self, v: Value) -> Result<i64, LuaError> {
        match self.len_value(v)? {
            Value::Int(i) => Ok(i),
            Value::Float(f) => crate::runtime::value::f2i_exact(f)
                .ok_or_else(|| self.rt_err("object length is not an integer")),
            _ => Err(self.rt_err("object length is not an integer")),
        }
    }

    pub(crate) fn index_value(&mut self, t: Value, key: Value) -> Result<Value, LuaError> {
        match self.index_step(t, key)? {
            MmOut::Done(v) => Ok(v),
            MmOut::Mm { func, recv } => self.call_mm1(func, &[recv, key]),
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from index_step"),
        }
    }

    /// Resolve `t[key]` through the `__index` chain, stopping at the first raw
    /// hit (`Done`) or function metamethod (`Mm`). Table-valued `__index` links
    /// are followed inline (no yield possible); only a function link can yield.
    fn index_step(&mut self, t: Value, key: Value) -> Result<MmOut, LuaError> {
        let mut cur = t;
        for _ in 0..MAX_TAG_LOOP {
            let mm = match cur {
                Value::Table(tb) => {
                    let v = tb.get(key);
                    if !v.is_nil() {
                        return Ok(MmOut::Done(v));
                    }
                    let mm = self.get_mm(cur, Mm::Index);
                    if mm.is_nil() {
                        return Ok(MmOut::Done(Value::Nil));
                    }
                    mm
                }
                v => {
                    let mm = self.get_mm(v, Mm::Index);
                    if mm.is_nil() {
                        return Err(self.type_err("index", v));
                    }
                    mm
                }
            };
            match mm {
                Value::Closure(_) | Value::Native(_) => {
                    return Ok(MmOut::Mm {
                        func: mm,
                        recv: cur,
                    });
                }
                next => cur = next,
            }
        }
        Err(self.rt_err("'__index' chain too long; possible loop"))
    }

    pub(crate) fn newindex_value(
        &mut self,
        t: Value,
        key: Value,
        v: Value,
    ) -> Result<(), LuaError> {
        match self.newindex_step(t, key, v)? {
            MmOut::Done(_) => Ok(()),
            MmOut::Mm { func, recv } => {
                self.call_value(func, &[recv, key, v])?;
                Ok(())
            }
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from newindex_step"),
        }
    }

    /// Resolve `t[key] = v` through the `__newindex` chain. A raw assignment is
    /// performed inline (returning `Done`); only a function metamethod (`Mm`)
    /// needs an actual call — which the caller may run yieldably.
    fn newindex_step(&mut self, t: Value, key: Value, v: Value) -> Result<MmOut, LuaError> {
        let mut cur = t;
        for _ in 0..MAX_TAG_LOOP {
            let mm = match cur {
                Value::Table(tb) => {
                    if !tb.get(key).is_nil() {
                        self.raw_set(tb, key, v)?;
                        return Ok(MmOut::Done(Value::Nil));
                    }
                    let mm = self.get_mm(cur, Mm::NewIndex);
                    if mm.is_nil() {
                        self.raw_set(tb, key, v)?;
                        return Ok(MmOut::Done(Value::Nil));
                    }
                    mm
                }
                bad => {
                    let mm = self.get_mm(bad, Mm::NewIndex);
                    if mm.is_nil() {
                        return Err(self.type_err("index", bad));
                    }
                    mm
                }
            };
            match mm {
                Value::Closure(_) | Value::Native(_) => {
                    return Ok(MmOut::Mm {
                        func: mm,
                        recv: cur,
                    });
                }
                next => cur = next,
            }
        }
        Err(self.rt_err("'__newindex' chain too long; possible loop"))
    }

    fn raw_set(&mut self, t: Gc<Table>, key: Value, v: Value) -> Result<(), LuaError> {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        match unsafe { t.as_mut() }.set(&mut self.heap, key, v) {
            Ok(()) => {
                self.heap
                    .barrier_back(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
                Ok(())
            }
            Err(TableError::NilIndex) => Err(self.rt_err("table index is nil")),
            Err(TableError::NanIndex) => Err(self.rt_err("table index is NaN")),
            Err(TableError::Overflow) => Err(self.rt_err("table overflow")),
            Err(TableError::InvalidNext) => unreachable!(),
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
            // PUC 5.2+ accepts any `__eq` reachable from either operand; 5.1
            // (and earlier) required the two operands' metatables to expose a
            // matching `__eq` (`get_compTM`) — `c == d` where `d` has no
            // metatable falls straight back to raw inequality. events.lua 5.1
            // :262 bakes this in.
            let mm = if self.version() <= LuaVersion::Lua51 {
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

    #[inline(always)]
    fn arith_rr(&mut self, inst: Inst, base: u32, op: ArithOp) -> Result<(), LuaError> {
        let l = self.r(base, inst.b());
        let r = self.r(base, inst.c());
        // hot path: Int + Int for Add / Sub / Mul — fib_28, loop_int_1m,
        // binary_trees all hammer these. Skipping coerce_num + the big
        // arith_fast match shaves several conditional moves per op.
        if let (Value::Int(a), Value::Int(b)) = (l, r) {
            let fast = match op {
                ArithOp::Add => Some(Value::Int(a.wrapping_add(b))),
                ArithOp::Sub => Some(Value::Int(a.wrapping_sub(b))),
                ArithOp::Mul => Some(Value::Int(a.wrapping_mul(b))),
                _ => None,
            };
            if let Some(v) = fast {
                self.set_r(base, inst.a(), v);
                return Ok(());
            }
        }
        // hot path: Float + Float for Add / Sub / Mul / Div — math_loop_100k
        // and any numeric workload with non-integer accumulators benefits.
        if let (Value::Float(a), Value::Float(b)) = (l, r) {
            let fast = match op {
                ArithOp::Add => Some(Value::Float(a + b)),
                ArithOp::Sub => Some(Value::Float(a - b)),
                ArithOp::Mul => Some(Value::Float(a * b)),
                ArithOp::Div => Some(Value::Float(a / b)),
                _ => None,
            };
            if let Some(v) = fast {
                self.set_r(base, inst.a(), v);
                return Ok(());
            }
        }
        match self.arith_fast(op, l, r)? {
            Some(v) => self.set_r(base, inst.a(), v),
            None => {
                let mm = self.arith_mm_func(op, l, r)?;
                let dst = base + inst.a();
                self.begin_meta_call(mm, &[l, r], MetaAction::Store { dst }, op.mm_name())?;
            }
        }
        Ok(())
    }

    /// Fast path for an arithmetic/bitwise op: `Ok(Some(v))` when computed
    /// directly, `Ok(None)` when a metamethod is required (the caller decides
    /// whether to call it synchronously or yieldably).
    fn arith_fast(&mut self, op: ArithOp, l: Value, r: Value) -> Result<Option<Value>, LuaError> {
        use ArithOp::*;
        match op {
            BAnd | BOr | BXor | Shl | Shr => {
                // strings coerce for bitwise too (PUC tointegerns via cvt2num)
                match (coerce_num(l), coerce_num(r)) {
                    (Some(a), Some(b)) => {
                        let to_int = |n: Num| match n {
                            Num::Int(i) => Some(i),
                            Num::Float(f) => crate::runtime::value::f2i_exact(f),
                        };
                        let (Some(a), Some(b)) = (to_int(a), to_int(b)) else {
                            // PUC luaG_tointerror: name the offending operand
                            return Err(self.no_int_rep_err());
                        };
                        let v = match op {
                            BAnd => a & b,
                            BOr => a | b,
                            BXor => a ^ b,
                            Shl => shift_left(a, b),
                            Shr => shift_left(a, b.wrapping_neg()),
                            _ => unreachable!(),
                        };
                        return Ok(Some(Value::Int(v)));
                    }
                    _ => return Ok(None),
                }
            }
            _ => {}
        }
        let (ln, rn) = match (coerce_num(l), coerce_num(r)) {
            (Some(a), Some(b)) => (a, b),
            _ => return Ok(None),
        };
        let v = match (op, ln, rn) {
            (Add, Num::Int(a), Num::Int(b)) => Value::Int(a.wrapping_add(b)),
            (Sub, Num::Int(a), Num::Int(b)) => Value::Int(a.wrapping_sub(b)),
            (Mul, Num::Int(a), Num::Int(b)) => Value::Int(a.wrapping_mul(b)),
            (IDiv, Num::Int(a), Num::Int(b)) => {
                if b == 0 {
                    return Err(self.rt_err("attempt to divide by zero"));
                }
                let mut q = a.wrapping_div(b);
                if (a ^ b) < 0 && q.wrapping_mul(b) != a {
                    q -= 1;
                }
                Value::Int(q)
            }
            (Mod, Num::Int(a), Num::Int(b)) => {
                if b == 0 {
                    return Err(self.rt_err("attempt to perform 'n%0'"));
                }
                let mut m = a.wrapping_rem(b);
                if m != 0 && (m ^ b) < 0 {
                    m += b;
                }
                Value::Int(m)
            }
            (Add, a, b) => Value::Float(a.as_f64() + b.as_f64()),
            (Sub, a, b) => Value::Float(a.as_f64() - b.as_f64()),
            (Mul, a, b) => Value::Float(a.as_f64() * b.as_f64()),
            (Div, a, b) => Value::Float(a.as_f64() / b.as_f64()),
            (Pow, a, b) => Value::Float(a.as_f64().powf(b.as_f64())),
            (IDiv, a, b) => Value::Float((a.as_f64() / b.as_f64()).floor()),
            (Mod, a, b) => {
                let (x, y) = (a.as_f64(), b.as_f64());
                // PUC luai_nummod: correct fmod's sign without the `m*y`
                // product, which underflows to 0 for tiny denormals
                let mut m = x % y;
                if (m > 0.0 && y < 0.0) || (m < 0.0 && y > 0.0) {
                    m += y;
                }
                Value::Float(m)
            }
            _ => unreachable!(),
        };
        Ok(Some(v))
    }

    pub(crate) fn int_from(&mut self, v: Value, what: &str) -> Result<i64, LuaError> {
        match v {
            Value::Int(i) => Ok(i),
            Value::Float(f) => match crate::runtime::value::f2i_exact(f) {
                Some(i) => Ok(i),
                None => Err(self.rt_err("number has no integer representation")),
            },
            v => Err(self.type_err(what, v)),
        }
    }

    fn int_from_num(&mut self, n: Num) -> Result<i64, LuaError> {
        match n {
            Num::Int(i) => Ok(i),
            Num::Float(f) => match crate::runtime::value::f2i_exact(f) {
                Some(i) => Ok(i),
                None => Err(self.rt_err("number has no integer representation")),
            },
        }
    }

    /// Find the arithmetic/bitwise metamethod (left operand first), or raise the
    /// PUC type error when neither operand provides one.
    fn arith_mm_func(&mut self, op: ArithOp, l: Value, r: Value) -> Result<Value, LuaError> {
        use ArithOp::*;
        let event = match op {
            Add => Mm::Add,
            Sub => Mm::Sub,
            Mul => Mm::Mul,
            Div => Mm::Div,
            Mod => Mm::Mod,
            Pow => Mm::Pow,
            IDiv => Mm::IDiv,
            BAnd => Mm::BAnd,
            BOr => Mm::BOr,
            BXor => Mm::BXor,
            Shl => Mm::Shl,
            Shr => Mm::Shr,
        };
        let mut mm = self.get_mm(l, event);
        if mm.is_nil() {
            mm = self.get_mm(r, event);
        }
        if mm.is_nil() {
            let what = if matches!(op, BAnd | BOr | BXor | Shl | Shr) {
                "perform bitwise operation on"
            } else {
                "perform arithmetic on"
            };
            let bad = if coerce_num(l).is_none() { l } else { r };
            return Err(self.type_err(what, bad));
        }
        Ok(mm)
    }

    // ---- comparison ----

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
                // PUC ≤5.3: `a <= b` falls back to `not (b < a)` when neither
                // operand carries `__le`. 5.4 dropped the synthesis (now
                // requires an explicit `__le`). events.lua 5.2/5.3 :172 relies
                // on the synthesis — its metatable defines only `__lt`.
                // The fallback calls `__lt(r, l)` synchronously (the suite's
                // `__lt` doesn't yield) and negates the result; the yieldable
                // `__lt` path stays reserved for the explicit `<` operator.
                if mm.is_nil() && or_eq && self.version <= crate::version::LuaVersion::Lua53 {
                    let lt = Mm::Lt;
                    let mut mm_lt = self.get_mm(l, lt);
                    if mm_lt.is_nil() {
                        mm_lt = self.get_mm(r, lt);
                    }
                    if !mm_lt.is_nil() {
                        return Ok(MmOut::CompareSynth { func: mm_lt });
                    }
                }
                if mm.is_nil() {
                    // PUC luaG_ordererror: "two X values" when the operand
                    // types match, "X with Y" otherwise (objtypename-aware).
                    let (t1, t2) = (self.obj_typename(l), self.obj_typename(r));
                    return Err(self.rt_err(&if t1 == t2 {
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

    fn for_prep(&mut self, inst: Inst, base: u32) -> Result<(), LuaError> {
        let a = inst.a();
        let init = self.r(base, a);
        let limit = self.r(base, a + 1);
        let step = self.r(base, a + 2);
        let (Some(init_n), Some(limit_n), Some(step_n)) =
            (as_num(init), as_num(limit), as_num(step))
        else {
            // PUC luaG_forerror: "bad 'for' <what> (number expected, got <type>)".
            // PUC checks limit, then step, then initial value.
            let (what, bad) = if as_num(limit).is_none() {
                ("limit", limit)
            } else if as_num(step).is_none() {
                ("step", step)
            } else {
                ("initial value", init)
            };
            let tn = self.obj_typename(bad);
            return Err(self.rt_err(&format!("bad 'for' {what} (number expected, got {tn})")));
        };
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
                if st == 0 {
                    return Err(self.rt_err("'for' step is zero"));
                }
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
                            if f.is_nan() {
                                (0, true)
                            } else if f >= i64::MAX as f64 + 1.0 {
                                // beyond +MAX: unreachable for a decreasing loop
                                (i64::MAX, st < 0)
                            } else if f <= i64::MIN as f64 {
                                // beyond -MIN: unreachable for an increasing loop
                                (i64::MIN, st >= 0)
                            } else if st > 0 {
                                (f.floor() as i64, false)
                            } else {
                                (f.ceil() as i64, false)
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
                if st == 0.0 {
                    return Err(self.rt_err("'for' step is zero"));
                }
                if pre53 {
                    let pre = x0 - st;
                    self.set_r(base, a, Value::Float(pre));
                    self.set_r(base, a + 1, Value::Float(lim));
                    self.set_r(base, a + 2, Value::Float(st));
                    self.add_pc(inst.bx() as i32 - 1);
                    return Ok(());
                }
                let runs = if st > 0.0 { x0 <= lim } else { x0 >= lim };
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
    fn for_loop(&mut self, inst: Inst, base: u32) {
        let a = inst.a();
        // PUC 5.1–5.3 `OP_FORLOOP` compares the post-step `i` to `limit`
        // directly (R[a+1] holds the limit, *not* a remaining-count) so the
        // first iteration's test fires through the same backward-jump path as
        // every later iteration. 5.4+ switched to the count-based form luna
        // already uses for `Int`; the float branch was already PUC-3.x-style.
        let pre53 = self.version() <= LuaVersion::Lua53;
        match self.r(base, a) {
            Value::Int(cur) if pre53 => {
                let Value::Int(lim) = self.r(base, a + 1) else {
                    unreachable!()
                };
                let Value::Int(st) = self.r(base, a + 2) else {
                    unreachable!()
                };
                let next = cur.wrapping_add(st);
                let cont = if st > 0 { next <= lim } else { next >= lim };
                if cont {
                    self.set_r(base, a, Value::Int(next));
                    self.set_r(base, a + 3, Value::Int(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            Value::Int(cur) => {
                let Value::Int(count) = self.r(base, a + 1) else {
                    unreachable!()
                };
                if count > 0 {
                    let Value::Int(st) = self.r(base, a + 2) else {
                        unreachable!()
                    };
                    let next = cur.wrapping_add(st);
                    self.set_r(base, a, Value::Int(next));
                    self.set_r(base, a + 1, Value::Int(count - 1));
                    self.set_r(base, a + 3, Value::Int(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            Value::Float(cur) => {
                let Value::Float(lim) = self.r(base, a + 1) else {
                    unreachable!()
                };
                let Value::Float(st) = self.r(base, a + 2) else {
                    unreachable!()
                };
                let next = cur + st;
                let cont = if st > 0.0 { next <= lim } else { next >= lim };
                if cont {
                    self.set_r(base, a, Value::Float(next));
                    self.set_r(base, a + 3, Value::Float(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            _ => unreachable!("corrupt for-loop state"),
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
        let legacy = self.version <= crate::version::LuaVersion::Lua52;
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
                        let legacy = self.version <= crate::version::LuaVersion::Lua52;
                        let bad = if concat_piece(x, legacy).is_none() {
                            x
                        } else {
                            y
                        };
                        return Err(self.type_err("concatenate", bad));
                    }
                    // result lands at i-1, dropping y (top→i); resume continues.
                    let dst = i - 1;
                    self.begin_meta_call(
                        mm,
                        &[x, y],
                        MetaAction::Concat { dst, base_a },
                        "concat",
                    )?;
                    return Ok(());
                }
            }
        }
        self.maybe_collect_garbage(base_a + 1);
        Ok(())
    }

    /// tostring with __tostring / __name support.
    pub(crate) fn tostring_value(&mut self, v: Value) -> Result<Vec<u8>, LuaError> {
        let mm = self.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            return match self.call_mm1(mm, &[v])? {
                Value::Str(s) => Ok(s.as_bytes().to_vec()),
                _ => Err(self.rt_err("'__tostring' must return a string")),
            };
        }
        if let Value::Table(t) = v
            && let Value::Str(name) = self.get_mm(v, Mm::Name)
        {
            let mut out = name.as_bytes().to_vec();
            out.extend_from_slice(format!(": {:p}", t.as_ptr()).as_bytes());
            return Ok(out);
        }
        Ok(self.tostring_basic(v))
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
                let legacy = self.version <= crate::version::LuaVersion::Lua52;
                numeric::num_to_string_for(Num::Float(f), legacy).into_bytes()
            }
            Value::Str(s) => s.as_bytes().to_vec(),
            Value::Table(t) => format!("table: {:p}", t.as_ptr()).into_bytes(),
            Value::Closure(c) => format!("function: {:p}", c.as_ptr()).into_bytes(),
            Value::Native(n) => format!("function: builtin: {:p}", n.as_ptr()).into_bytes(),
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Mod,
    Pow,
    Div,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
}

impl ArithOp {
    /// PUC metamethod event name (`__add` → "add" etc.) used by
    /// `debug.getinfo(level, "n")` inside a metamethod handler.
    fn mm_name(self) -> &'static str {
        match self {
            ArithOp::Add => "add",
            ArithOp::Sub => "sub",
            ArithOp::Mul => "mul",
            ArithOp::Mod => "mod",
            ArithOp::Pow => "pow",
            ArithOp::Div => "div",
            ArithOp::IDiv => "idiv",
            ArithOp::BAnd => "band",
            ArithOp::BOr => "bor",
            ArithOp::BXor => "bxor",
            ArithOp::Shl => "shl",
            ArithOp::Shr => "shr",
        }
    }
}

fn as_num(v: Value) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        // PUC forprep coerces numeric strings (`for i = "10", "1", "-2"`).
        Value::Str(s) => crate::numeric::str2num(s.as_bytes(), true, true),
        _ => None,
    }
}

/// A concatenable operand's byte form (string, or a number coerced to its
/// string), or `None` when only a `__concat` metamethod can handle it.
/// `legacy_float = true` follows PUC ≤5.2's `%.14g` rendering (no `.0`
/// suffix on integer-valued floats) — see `num_to_string_for`.
fn concat_piece(v: Value, legacy_float: bool) -> Option<Vec<u8>> {
    match v {
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        Value::Int(x) => Some(numeric::num_to_string(Num::Int(x)).into_bytes()),
        Value::Float(x) => {
            Some(numeric::num_to_string_for(Num::Float(x), legacy_float).into_bytes())
        }
        _ => None,
    }
}

/// Index into the per-basic-type metatable table for a non-table value
/// (None for tables, which carry their own metatable).
fn type_mt_slot(v: Value) -> Option<usize> {
    match v {
        Value::Nil => Some(0),
        Value::Bool(_) => Some(1),
        Value::Int(_) | Value::Float(_) => Some(2),
        Value::Str(_) => Some(3),
        Value::Closure(_) | Value::Native(_) => Some(4),
        // tables and full userdata carry their own metatable; threads and
        // light userdata have none (PUC keeps a shared per-type mt slot for
        // light, but luna doesn't expose it — no test gates on it yet).
        Value::Table(_) | Value::Coro(_) | Value::Userdata(_) | Value::LightUserdata(_) => None,
    }
}

/// Number, or string coerced to number (5.5 default string-arith coercion).
fn coerce_num(v: Value) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        Value::Str(s) => numeric::str2num(s.as_bytes(), true, true),
        _ => None,
    }
}

/// Lua shifts: logical on 64 bits; |shift| ≥ 64 yields 0; negative shifts
/// reverse direction.
fn shift_left(a: i64, b: i64) -> i64 {
    if b < 0 {
        if b <= -64 {
            0
        } else {
            ((a as u64) >> (-b as u32)) as i64
        }
    } else if b >= 64 {
        0
    } else {
        ((a as u64) << (b as u32)) as i64
    }
}

/// i < f, exactly (PUC LTintfloat shape).
fn int_lt_float(i: i64, f: f64) -> bool {
    if f.is_nan() {
        return false;
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return true;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return false;
    }
    let ff = f.floor();
    let fi = ff as i64;
    if f == ff { i < fi } else { i <= fi }
}

/// i <= f, exactly.
fn int_le_float(i: i64, f: f64) -> bool {
    if f.is_nan() {
        return false;
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return true;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return false;
    }
    i <= f.floor() as i64
}

/// Clip a numeric `for` limit to the integer range (PUC forlimit). Returns
/// (clipped limit, loop-is-empty).
fn int_for_limit(limit: Num, init: i64, step: i64) -> (i64, bool) {
    match limit {
        Num::Int(l) => {
            let empty = if step > 0 { init > l } else { init < l };
            (l, empty)
        }
        Num::Float(f) => {
            if f.is_nan() {
                return (0, true);
            }
            if step > 0 {
                if f >= 9_223_372_036_854_775_808.0 {
                    (i64::MAX, false)
                } else {
                    let l = f.floor();
                    if l < -9_223_372_036_854_775_808.0 {
                        (i64::MIN, true)
                    } else {
                        let li = l as i64;
                        (li, init > li)
                    }
                }
            } else if f <= -9_223_372_036_854_775_808.0 {
                (i64::MIN, false)
            } else {
                let l = f.ceil();
                if l >= 9_223_372_036_854_775_808.0 {
                    // PUC forlimit: a positive limit beyond the integer range
                    // is unreachable for a decreasing loop — empty.
                    (i64::MAX, true)
                } else {
                    let li = l as i64;
                    (li, init < li)
                }
            }
        }
    }
}

/// Strip the load-prefix sigil from a chunk name for messages (PUC keeps
/// `@file` / `=name` markers in `source`).
fn chunk_display_name(p: *const crate::runtime::LuaStr) -> &'static [u8] {
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let b = unsafe { crate::runtime::string::bytes_of(p) };
    match b.first() {
        Some(b'@') | Some(b'=') => &b[1..],
        _ => b,
    }
}

impl Vm {
    /// Frame introspection for debug.getinfo: `level` 1 = the Lua function
    /// that called the current native. Returns (closure, current line,
    /// extra vararg count).
    /// Name (and kind: local/global/field/upvalue/method/for iterator) of the
    /// function running at `level`, recovered from the caller's call
    /// instruction (PUC funcnamefromcode). None for the main chunk or a
    /// tail/anonymous call with no recoverable name.
    /// A debug-level position: either a real Lua frame (by index) or a synthetic
    /// C frame standing for a call_value boundary (metamethod / pcall / __close /
    /// coroutine body), which `debug.getinfo` and traceback report as "C".
    /// PUC lua_getlocal: the `n`-th (1-based) local variable active at the Lua
    /// frame at `level`'s current pc, as (name, value). Locals are visited in
    /// registration order (start pc, then register) to match luaF_getlocalname.
    pub(crate) fn local_at(&self, level: i64, n: i64) -> Option<(String, Value)> {
        if n == 0 {
            return None;
        }
        let fi = match self.dbg_frame(level)? {
            DbgKind::Lua(fi) => fi,
            // Tail-call placeholder has no real frame backing it — no locals
            // exist to read or write here. PUC `findlocal` returns NULL on
            // a CIST_TAIL activation.
            DbgKind::Tail(_) => return None,
            // PUC's `luaG_findlocal` on a C activation returns `(C temporary)`
            // for slot `n` inside the argument window (db.lua :408-:413, and
            // the call/return hook reads of math.sin / select args via
            // `getinfo("r")` + `getlocal`). Negative `n` (vararg) is not
            // meaningful for a C frame here.
            DbgKind::C(fi) => {
                if n < 1 {
                    return None;
                }
                let (func_slot, nargs) = self.c_frame_native_slots(fi)?;
                if (n as u32) > nargs {
                    return None;
                }
                let slot = (func_slot + n as u32) as usize;
                let val = self.stack.get(slot).copied().unwrap_or(Value::Nil);
                return Some((self.temporary_locvar_name().to_string(), val));
            }
        };
        let f = self.frames[fi].lua()?;
        // PUC `lua_getlocal` with a negative `n` indexes the varargs: `-1`
        // is the first extra arg passed to the function (`...[1]`), `-2` the
        // second, etc. The 5.5 stack layout parks varargs in
        // [func_slot + 1, base), so the i-th is at `func_slot + i`.
        if n < 0 {
            let i = (-n) as u32;
            if i == 0 || i > f.n_varargs {
                return None;
            }
            let val = self
                .stack
                .get((f.func_slot + i) as usize)
                .copied()
                .unwrap_or(Value::Nil);
            return Some((self.vararg_locvar_name().to_string(), val));
        }
        let proto = f.closure.proto;
        // PUC's parser injects a hidden `(vararg table)` locvar for an
        // anonymous-vararg function (lparser.c new_localvarliteral), sitting
        // right after the fixed parameters (`numparams + 1`). Main chunks
        // and `(...t)` named-vararg funcs do NOT get one — gate on the
        // compiler-set flag, not on `is_vararg`. luna keeps user locals in
        // their declared registers (no shadow slot allocated), so we expose
        // that hidden index purely in this debug view.
        let num_params = proto.num_params as i64;
        let vararg_slot = if proto.has_vararg_table_pseudo {
            Some(num_params + 1)
        } else {
            None
        };
        if vararg_slot == Some(n) {
            return Some(("(vararg table)".to_string(), Value::Nil));
        }
        let pc = (f.pc as usize).saturating_sub(1);
        let mut active: Vec<&crate::runtime::LocVar> = proto
            .locvars
            .iter()
            .filter(|lv| (lv.start_pc as usize) <= pc && pc < lv.end_pc as usize)
            .collect();
        active.sort_by_key(|lv| (lv.start_pc, lv.reg));
        let mut idx: i64 = n - 1;
        if let Some(vs) = vararg_slot
            && n > vs
        {
            idx -= 1;
        }
        let idx = idx as usize;
        if let Some(lv) = active.get(idx) {
            let val = self
                .stack
                .get((f.base + lv.reg) as usize)
                .copied()
                .unwrap_or(Value::Nil);
            return Some((lv.name.to_string(), val));
        }
        // PUC `luaG_findlocal` fallback: `n` is past the named locals but
        // still inside the frame's live register window — report a
        // "(temporary)" (e.g. an arithmetic intermediate). The limit is
        // the next frame's func slot (`ci->next->func.p`) so the
        // temporary window stops where the callee's frame begins
        // (db.lua :416/:417 distinguish a live temporary `(a+1)` from
        // an out-of-range slot).
        let limit = self
            .frames
            .get(fi + 1)
            .and_then(|cf| cf.lua())
            .map(|nf| nf.func_slot)
            .unwrap_or_else(|| self.top.max(f.base));
        let temp_reg = idx as u32;
        if f.base + temp_reg < limit {
            let val = self
                .stack
                .get((f.base + temp_reg) as usize)
                .copied()
                .unwrap_or(Value::Nil);
            return Some((self.lua_temporary_locvar_name().to_string(), val));
        }
        None
    }

    /// `debug.setlocal`'s underlying write (PUC `lua_setlocal`). Returns
    /// the local / vararg name on success, `None` when the slot does not
    /// resolve. Mirrors `local_at`'s indexing exactly.
    pub(crate) fn local_set(&mut self, level: i64, n: i64, v: Value) -> Option<String> {
        if n == 0 {
            return None;
        }
        let DbgKind::Lua(fi) = self.dbg_frame(level)? else {
            return None;
        };
        let f = self.frames[fi].lua()?;
        if n < 0 {
            let i = (-n) as u32;
            if i == 0 || i > f.n_varargs {
                return None;
            }
            let slot = (f.func_slot + i) as usize;
            if let Some(s) = self.stack.get_mut(slot) {
                *s = v;
            }
            return Some(self.vararg_locvar_name().to_string());
        }
        let proto = f.closure.proto;
        let num_params = proto.num_params as i64;
        let vararg_slot = if proto.has_vararg_table_pseudo {
            Some(num_params + 1)
        } else {
            None
        };
        if vararg_slot == Some(n) {
            // hidden (vararg table) slot has no real storage — accept the
            // write as a no-op for PUC parity (db.lua doesn't write to it).
            return Some("(vararg table)".to_string());
        }
        let pc = (f.pc as usize).saturating_sub(1);
        let mut active: Vec<&crate::runtime::LocVar> = proto
            .locvars
            .iter()
            .filter(|lv| (lv.start_pc as usize) <= pc && pc < lv.end_pc as usize)
            .collect();
        active.sort_by_key(|lv| (lv.start_pc, lv.reg));
        let mut idx: i64 = n - 1;
        if let Some(vs) = vararg_slot
            && n > vs
        {
            idx -= 1;
        }
        let idx = idx as usize;
        let (name, reg) = if let Some(lv) = active.get(idx) {
            (lv.name.to_string(), lv.reg)
        } else {
            // PUC `luaG_findlocal` fallback into the temporary window —
            // bounded by the next frame's func slot (see local_at).
            let limit = self
                .frames
                .get(fi + 1)
                .and_then(|cf| cf.lua())
                .map(|nf| nf.func_slot)
                .unwrap_or_else(|| self.top.max(f.base));
            let temp_reg = idx as u32;
            if f.base + temp_reg >= limit {
                return None;
            }
            (self.lua_temporary_locvar_name().to_string(), temp_reg)
        };
        let slot = (f.base + reg) as usize;
        if let Some(s) = self.stack.get_mut(slot) {
            *s = v;
        }
        Some(name)
    }

    /// `debug.getlocal(thread, level, n)`: read frame `level` of the suspended
    /// coroutine `co`. Walks `co.frames` (the saved Lua activation stack) and
    /// reads from `co.stack`. Returns `None` for out-of-range, for negative
    /// vararg indexing past `n_varargs`, or for a register past the live
    /// window. Naming follows the same priority as `local_at`: named locals,
    /// then `(vararg)` for negative `n`, then `(vararg table)` for the
    /// explicit-`(...)` pseudo, else `(temporary)` in the live register
    /// window.
    pub(crate) fn local_at_coro(
        &self,
        co: Gc<crate::runtime::Coro>,
        level: i64,
        n: i64,
    ) -> Option<(String, Value)> {
        if level < 1 || n == 0 {
            return None;
        }
        let frames = &co.frames;
        // Logical level: iterate Lua frames from the top.
        let lua_indices: Vec<usize> = (0..frames.len())
            .rev()
            .filter(|&i| frames[i].lua().is_some())
            .collect();
        let fi = *lua_indices.get((level - 1) as usize)?;
        let f = frames[fi].lua()?;
        if n < 0 {
            let i = (-n) as u32;
            if i == 0 || i > f.n_varargs {
                return None;
            }
            let val = co
                .stack
                .get((f.func_slot + i) as usize)
                .copied()
                .unwrap_or(Value::Nil);
            return Some((self.vararg_locvar_name().to_string(), val));
        }
        let proto = f.closure.proto;
        let num_params = proto.num_params as i64;
        let vararg_slot = if proto.has_vararg_table_pseudo {
            Some(num_params + 1)
        } else {
            None
        };
        if vararg_slot == Some(n) {
            return Some(("(vararg table)".to_string(), Value::Nil));
        }
        let pc = (f.pc as usize).saturating_sub(1);
        let mut active: Vec<&crate::runtime::LocVar> = proto
            .locvars
            .iter()
            .filter(|lv| (lv.start_pc as usize) <= pc && pc < lv.end_pc as usize)
            .collect();
        active.sort_by_key(|lv| (lv.start_pc, lv.reg));
        let mut idx: i64 = n - 1;
        if let Some(vs) = vararg_slot
            && n > vs
        {
            idx -= 1;
        }
        let idx = idx as usize;
        if let Some(lv) = active.get(idx) {
            let val = co
                .stack
                .get((f.base + lv.reg) as usize)
                .copied()
                .unwrap_or(Value::Nil);
            return Some((lv.name.to_string(), val));
        }
        let limit = frames
            .get(fi + 1)
            .and_then(|cf| cf.lua())
            .map(|nf| nf.func_slot)
            .unwrap_or(co.top.max(f.base));
        let temp_reg = idx as u32;
        if f.base + temp_reg < limit {
            let val = co
                .stack
                .get((f.base + temp_reg) as usize)
                .copied()
                .unwrap_or(Value::Nil);
            return Some((self.lua_temporary_locvar_name().to_string(), val));
        }
        None
    }

    /// `debug.setlocal(thread, level, n, value)`: write into frame `level` of
    /// suspended `co`. Mirrors `local_at_coro`'s indexing exactly.
    pub(crate) fn local_set_coro(
        &mut self,
        co: Gc<crate::runtime::Coro>,
        level: i64,
        n: i64,
        v: Value,
    ) -> Option<String> {
        if level < 1 || n == 0 {
            return None;
        }
        let lua_indices: Vec<usize> = (0..co.frames.len())
            .rev()
            .filter(|&i| co.frames[i].lua().is_some())
            .collect();
        let fi = *lua_indices.get((level - 1) as usize)?;
        let (func_slot, n_varargs, base, proto, top_for_temp, next_func_slot) = {
            let f = co.frames[fi].lua()?;
            (
                f.func_slot,
                f.n_varargs,
                f.base,
                f.closure.proto,
                co.top.max(f.base),
                co.frames
                    .get(fi + 1)
                    .and_then(|cf| cf.lua())
                    .map(|nf| nf.func_slot),
            )
        };
        if n < 0 {
            let i = (-n) as u32;
            if i == 0 || i > n_varargs {
                return None;
            }
            let slot = (func_slot + i) as usize;
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            let stack = unsafe { &mut co.as_mut().stack };
            if let Some(s) = stack.get_mut(slot) {
                *s = v;
            }
            // co.stack values are traced — once-per-call barrier so propagate
            // sees the new value if co was already BLACK this cycle.
            self.heap
                .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
            return Some(self.vararg_locvar_name().to_string());
        }
        let num_params = proto.num_params as i64;
        let vararg_slot = if proto.has_vararg_table_pseudo {
            Some(num_params + 1)
        } else {
            None
        };
        if vararg_slot == Some(n) {
            return Some("(vararg table)".to_string());
        }
        let pc = (co.frames[fi].lua().unwrap().pc as usize).saturating_sub(1);
        let mut active: Vec<&crate::runtime::LocVar> = proto
            .locvars
            .iter()
            .filter(|lv| (lv.start_pc as usize) <= pc && pc < lv.end_pc as usize)
            .collect();
        active.sort_by_key(|lv| (lv.start_pc, lv.reg));
        let mut idx: i64 = n - 1;
        if let Some(vs) = vararg_slot
            && n > vs
        {
            idx -= 1;
        }
        let idx = idx as usize;
        let (name, reg) = if let Some(lv) = active.get(idx) {
            (lv.name.to_string(), lv.reg)
        } else {
            let limit = next_func_slot.unwrap_or(top_for_temp);
            let temp_reg = idx as u32;
            if base + temp_reg >= limit {
                return None;
            }
            (self.lua_temporary_locvar_name().to_string(), temp_reg)
        };
        let slot = (base + reg) as usize;
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let stack = unsafe { &mut co.as_mut().stack };
        if let Some(s) = stack.get_mut(slot) {
            *s = v;
        }
        // co.stack values are traced — once-per-call barrier so propagate
        // sees the new value if co was already BLACK this cycle.
        self.heap
            .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
        Some(name)
    }

    /// Frame info for a level on a suspended coroutine (PUC
    /// `lua_getinfo(L1, "Sl...", &ar)` after `lua_getstack(L1, level, &ar)`).
    /// Returns the closure + currentline + extraargs + istailcall for the
    /// level-th Lua activation in `co.frames`. None if level overshoots.
    pub(crate) fn coro_frame_info(
        &self,
        co: Gc<crate::runtime::Coro>,
        level: i64,
    ) -> Option<(Gc<LuaClosure>, u32, i64, bool)> {
        if level < 1 {
            return None;
        }
        let lua_indices: Vec<usize> = (0..co.frames.len())
            .rev()
            .filter(|&i| co.frames[i].lua().is_some())
            .collect();
        let fi = *lua_indices.get((level - 1) as usize)?;
        let f = co.frames[fi].lua()?;
        let proto = f.closure.proto;
        let pc = (f.pc as usize)
            .saturating_sub(1)
            .min(proto.lines.len().saturating_sub(1));
        let line = proto.lines.get(pc).copied().unwrap_or(0);
        Some((f.closure, line, f.n_varargs as i64, f.tailcalls > 0))
    }

    /// Whether `level` resolves to any live activation (PUC lua_getstack).
    pub(crate) fn level_in_range(&self, level: i64) -> bool {
        self.dbg_frame(level).is_some()
    }

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

    /// The Lua closure running at `level` on the current thread, or `None`
    /// when the frame is a synthetic C boundary. PUC 5.1 `getfenv`/`setfenv`
    /// need this to reach the function whose env they read or rewrite.
    pub(crate) fn lua_closure_at_level(&self, level: i64) -> Option<Gc<LuaClosure>> {
        // `DbgKind::Tail` also falls into the else branch — a tail-call
        // placeholder has no closure of its own, so PUC's `lua_getstack` +
        // `getfunc` for that level returns no function, and `getfenv(level)`
        // / `setfenv(level)` raise an error (5.1 db.lua :336/:341).
        let DbgKind::Lua(fi) = self.dbg_frame(level)? else {
            return None;
        };
        Some(self.frames[fi].lua()?.closure)
    }

    pub(crate) fn coro_level_in_range(&self, co: Gc<crate::runtime::Coro>, level: i64) -> bool {
        if level < 1 {
            return false;
        }
        let count = co.frames.iter().filter(|cf| cf.lua().is_some()).count();
        (level as usize) <= count
    }

    pub(crate) fn dbg_frame(&self, level: i64) -> Option<DbgKind> {
        if level < 1 {
            return None;
        }
        // PUC 5.1's `lua_getstack` walks the full `ci` chain — each C
        // activation counts as a level, and each Lua activation's
        // `tailcalls` adds an extra synthetic level (CIST_TAIL). 5.2+
        // dropped the synthetic shape: `istailcall` becomes a flag on the
        // real frame and Cont activations no longer count separately.
        // 5.1 db.lua :336-:343 pin the 5.1 shape; 5.2/5.3/5.5 db.lua's
        // `getinfo(2).func == g1` pins the 5.2+ shape.
        let v51 = self.version <= LuaVersion::Lua51;
        let mut lvl = level;
        for fi in (0..self.frames.len()).rev() {
            match &self.frames[fi] {
                CallFrame::Lua(f) => {
                    lvl -= 1;
                    if lvl == 0 {
                        return Some(DbgKind::Lua(fi));
                    }
                    if v51 {
                        // 5.1 reports one synthetic CIST_TAIL level per
                        // collapsed tail call (PUC `lua_getstack` subtracts
                        // `ci->u.l.tailcalls` from the remaining level).
                        for _ in 0..f.tailcalls {
                            lvl -= 1;
                            if lvl == 0 {
                                return Some(DbgKind::Tail(fi));
                            }
                        }
                    }
                    if f.from_c {
                        lvl -= 1;
                        if lvl == 0 {
                            return Some(DbgKind::C(fi));
                        }
                    }
                }
                CallFrame::Cont(_) => {
                    if !v51 {
                        continue;
                    }
                    lvl -= 1;
                    if lvl == 0 {
                        let parent = (0..fi)
                            .rev()
                            .find(|&j| matches!(self.frames[j], CallFrame::Lua(_)));
                        return Some(DbgKind::C(parent.unwrap_or(fi.saturating_sub(1))));
                    }
                }
            }
        }
        None
    }

    pub(crate) fn frame_name(&self, fi: usize) -> Option<(&'static str, String)> {
        let f = self.frames[fi].lua()?;
        // metamethod handler frames carry the event tag (e.g. "close" for
        // `__close`); PUC `funcnamefromcall` reads `ci->u.l.tm`.
        if f.is_hook {
            return Some(("hook", "?".to_string()));
        }
        if let Some(tm) = f.tm {
            return Some(("metamethod", tm_debug_name(self.version, tm)));
        }
        // a frame entered across a C boundary has no naming call instruction
        if fi == 0 || f.from_c {
            return None;
        }
        // the caller's call instruction names this frame; a continuation frame
        // just below (pcall/xpcall) is itself a C boundary, so f.from_c above
        // already short-circuits those.
        let caller = self.frames[fi - 1].lua()?;
        let caller_proto = caller.closure.proto;
        let p: &crate::runtime::Proto = &caller_proto;
        let call_pc = (caller.pc as usize).checked_sub(1)?;
        let instr = *p.code.get(call_pc)?;
        match instr.op() {
            Op::Call | Op::TailCall => crate::vm::objname::getobjname(p, call_pc, instr.a()),
            Op::TForCall => Some(("for iterator", "for iterator".to_string())),
            _ => None,
        }
    }

    /// Name the synthetic C level sitting below the `from_c` Lua frame at `fi`
    /// (PUC names a C function from the call instruction that invoked it). The
    /// native was called by the nearest Lua frame below `fi` (skipping pcall/
    /// xpcall continuations); that frame's call instruction names it.
    pub(crate) fn c_frame_name(&self, fi: usize) -> Option<(&'static str, String)> {
        // PUC `GCTM` sets `CIST_FIN` on the calling ci, so when getinfo names
        // the synthetic C edge between the __gc finalizer (top Lua frame, has
        // `tm = "gc"`) and its triggering Lua frame it reports "metamethod"
        // "__gc" — 5.3 db.lua :720's `getinfo(2).namewhat == "metamethod"`
        // pin. Restricted to the `__gc` event: `__close` (`tm = "close"`)
        // sets the tag on the handler frame only, so level 2 there still
        // names the calling Lua frame's call instruction (5.5 locals.lua
        // :514 pins `getinfo(2).name == "pcall"` from a __close handler).
        if let Some(fr) = self.frames.get(fi).and_then(|cf| cf.lua())
            && fr.tm == Some("gc")
        {
            let name = tm_debug_name(self.version, "gc");
            return Some(("metamethod", name));
        }
        let caller_fi = (0..fi).rev().find(|&i| self.frames[i].lua().is_some())?;
        let caller = self.frames[caller_fi].lua()?;
        let p = &caller.closure.proto;
        let call_pc = (caller.pc as usize).checked_sub(1)?;
        let instr = *p.code.get(call_pc)?;
        match instr.op() {
            Op::Call | Op::TailCall => crate::vm::objname::getobjname(p, call_pc, instr.a()),
            _ => None,
        }
    }

    /// Native value currently sitting on the synthetic C edge identified by
    /// `DbgKind::C(fi)`. The walk counts how many `from_c` Lua frames live
    /// above `fi` (each one corresponds to one native pushing the hook) and
    /// indexes into `running_natives` from the top, also skipping the caller
    /// of `getinfo` itself (the native that is currently asking).
    /// db.lua :344 reads `debug.getinfo(2, "f").func` from a call hook and
    /// expects the just-entered C function.
    pub(crate) fn c_frame_func(&self, fi: usize) -> Option<Value> {
        let idx = self.c_frame_native_idx(fi)?;
        Some(Value::Native(self.running_natives[idx]))
    }

    /// `(func_slot, nargs)` for the synthetic C edge identified by `C(fi)`,
    /// so `local_at` can index the native's argument window like PUC's
    /// `(C temporary)` path. Returns `None` when no matching native exists
    /// (e.g. the C edge corresponds to a non-native boundary).
    pub(crate) fn c_frame_native_slots(&self, fi: usize) -> Option<(u32, u32)> {
        let idx = self.c_frame_native_idx(fi)?;
        self.running_native_slots.get(idx).copied()
    }

    fn c_frame_native_idx(&self, fi: usize) -> Option<usize> {
        let n_above = self.frames[fi..]
            .iter()
            .filter_map(CallFrame::lua)
            .filter(|f| f.from_c)
            .count();
        if n_above == 0 {
            return None;
        }
        // running_natives.last() is the native currently executing (the one
        // that called getinfo). Pop it conceptually, then take the n_above-th
        // entry from the top of what remains.
        let nr = self.running_natives.len().checked_sub(1)?;
        nr.checked_sub(n_above)
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

    /// Name and namewhat of the native currently running on behalf of the top
    /// Lua frame's call instruction (PUC `lua_getinfo("n")` at level 0). Lets
    /// `luaL_argerror` rewrite a method call's self-argument error.
    pub(crate) fn running_call_name(&self) -> Option<(&'static str, String)> {
        let caller = self.frames.iter().rev().find_map(CallFrame::lua)?;
        let p = &caller.closure.proto;
        let call_pc = (caller.pc as usize).checked_sub(1)?;
        let instr = *p.code.get(call_pc)?;
        match instr.op() {
            Op::Call | Op::TailCall => crate::vm::objname::getobjname(p, call_pc, instr.a()),
            _ => None,
        }
    }

    pub(crate) fn frame_info(&mut self, fi: usize) -> (Gc<LuaClosure>, u32, i64, bool) {
        let f = self.frames[fi].lua().expect("Lua frame");
        let proto = f.closure.proto;
        let pc = (f.pc as usize)
            .saturating_sub(1)
            .min(proto.lines.len().saturating_sub(1));
        let line = proto.lines.get(pc).copied().unwrap_or(0);
        // PUC CallInfo.nextraargs: the original extra-arg count, fixed at call
        // (independent of any later write to a materialized vararg table's `n`).
        // `istailcall` mirrors PUC `CIST_TAIL` for `debug.getinfo(_, "t")` —
        // any nonzero `tailcalls` count flips it true.
        (f.closure, line, f.n_varargs as i64, f.tailcalls > 0)
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

    /// Lines for debug.traceback (PUC `luaL_traceback` / `pushfuncname`).
    /// Per Lua frame, emits `"\n\t<src>:<line>: in <funcname>"` where
    /// `<funcname>` is, in priority order: `"metamethod 'event'"` if the frame
    /// is a metamethod handler (e.g. `__close`); else `"<namewhat> '<name>'"`
    /// from the caller's call instruction (`getobjname`); else `"main chunk"`;
    /// else `"function <src:line_defined>"` for an anonymous Lua function.
    /// Traceback of a suspended coroutine (PUC `debug.traceback(L1, msg, lvl)`).
    /// Walks the coroutine's saved frames and prepends a synthetic C-level
    /// `'yield'` entry when the coroutine paused at a `coroutine.yield` call
    /// (its `resume_at` marker is set). `level` skips entries from the top
    /// (level 0 includes the yield frame; level 1 starts at the deepest Lua
    /// frame; etc.). db.lua :764-:768 sample several levels.
    pub(crate) fn coro_traceback(&self, co: Gc<crate::runtime::Coro>, mut level: i64) -> Vec<u8> {
        use crate::runtime::CoroStatus;
        const LEVELS1: usize = 10;
        const LEVELS2: usize = 11;
        #[derive(Clone, Copy)]
        enum VFrame<'a> {
            Lua(&'a crate::runtime::function::Frame),
            CPcall,
            CXpcall,
            CYield,
            /// Synthetic CIST_TAIL placeholder under 5.1 — one per tail
            /// call collapsed into the next Lua frame down the chain.
            Tail,
        }
        let v51 = self.version <= LuaVersion::Lua51;
        let mut visible: Vec<VFrame<'_>> = Vec::new();
        // PUC's level 0 entry on a suspended coroutine is the C call where it
        // paused — `coroutine.yield` for a yielded thread.
        if matches!(co.status, CoroStatus::Suspended) && co.resume_at.is_some() {
            visible.push(VFrame::CYield);
        }
        for cf in co.frames.iter().rev() {
            match cf {
                CallFrame::Lua(f) => {
                    visible.push(VFrame::Lua(f));
                    if v51 {
                        for _ in 0..f.tailcalls {
                            visible.push(VFrame::Tail);
                        }
                    }
                }
                CallFrame::Cont(nc) => match nc.kind {
                    ContKind::Pcall => visible.push(VFrame::CPcall),
                    ContKind::Xpcall { .. } => visible.push(VFrame::CXpcall),
                    _ => {}
                },
            }
        }
        if level < 0 {
            level = 0;
        }
        if (level as usize) >= visible.len() {
            return Vec::new();
        }
        let visible = &visible[level as usize..];
        let total = visible.len();
        let mut out = Vec::new();
        // To name a Lua frame, PUC consults the caller's OP_CALL via
        // getobjname: find the index `fi` of the current frame in co.frames,
        // then look at frames[fi-1] (the caller) and read its `code[pc-1]`.
        let coro_frame_name = |frames: &[CallFrame],
                               target: &crate::runtime::function::Frame|
         -> Option<(&'static str, String)> {
            let fi = frames
                .iter()
                .position(|cf| matches!(cf, CallFrame::Lua(f) if std::ptr::eq(f, target)))?;
            if fi == 0 || target.from_c {
                return None;
            }
            let caller = frames[fi - 1].lua()?;
            let p = &caller.closure.proto;
            let call_pc = (caller.pc as usize).checked_sub(1)?;
            let instr = *p.code.get(call_pc)?;
            match instr.op() {
                Op::Call | Op::TailCall => crate::vm::objname::getobjname(p, call_pc, instr.a()),
                Op::TForCall => Some(("for iterator", "for iterator".to_string())),
                _ => None,
            }
        };
        let frames = &co.frames;
        let emit = |out: &mut Vec<u8>, v: VFrame<'_>| match v {
            VFrame::Lua(f) => {
                let proto = f.closure.proto;
                let src = chunk_display_name(proto.source.as_ptr());
                let pc = (f.pc as usize)
                    .saturating_sub(1)
                    .min(proto.lines.len().saturating_sub(1));
                let line = proto.lines.get(pc).copied().unwrap_or(0);
                out.extend_from_slice(b"\n\t");
                out.extend_from_slice(src);
                out.extend_from_slice(format!(":{line}: in ").as_bytes());
                if let Some((namewhat, name)) = coro_frame_name(frames, f) {
                    out.extend_from_slice(format!("{namewhat} '{name}'").as_bytes());
                } else if proto.line_defined == 0 {
                    out.extend_from_slice(b"main chunk");
                } else {
                    out.extend_from_slice(
                        format!(
                            "function <{}:{}>",
                            String::from_utf8_lossy(src),
                            proto.line_defined
                        )
                        .as_bytes(),
                    );
                }
            }
            VFrame::CPcall => out.extend_from_slice(b"\n\t[C]: in function 'pcall'"),
            VFrame::CXpcall => out.extend_from_slice(b"\n\t[C]: in function 'xpcall'"),
            VFrame::CYield => {
                // PUC `pushglobalfuncname` reports `yield` as
                // `'coroutine.yield'` under 5.3 and 5.4 (5.3 :566 / 5.4 :830
                // `checktraceback` baselines). 5.1/5.2/5.5 emit the bare
                // `'yield'` (5.5 :841).
                let qualified = matches!(self.version, LuaVersion::Lua53 | LuaVersion::Lua54);
                if qualified {
                    out.extend_from_slice(b"\n\t[C]: in function 'coroutine.yield'");
                } else {
                    out.extend_from_slice(b"\n\t[C]: in function 'yield'");
                }
            }
            VFrame::Tail => {
                // 5.1 traceback synthetic CIST_TAIL entry — luaG_addinfo
                // / luaO_chunkid format: `(...tail calls...)`. 5.1 db.lua
                // :403 asserts these appear once per collapsed tail call.
                out.extend_from_slice(b"\n\t(...tail calls...)");
            }
        };
        if total <= LEVELS1 + LEVELS2 {
            for &v in visible {
                emit(&mut out, v);
            }
        } else {
            for &v in &visible[..LEVELS1] {
                emit(&mut out, v);
            }
            let skip = total - LEVELS1 - LEVELS2;
            out.extend_from_slice(format!("\n\t...\t(skipping {skip} levels)").as_bytes());
            for &v in &visible[total - LEVELS2..] {
                emit(&mut out, v);
            }
        }
        out
    }

    pub(crate) fn traceback_bytes(&self, level: i64) -> Vec<u8> {
        // PUC `luaL_traceback` shows up to LEVELS1 (10) top frames + LEVELS2
        // (11) bottom frames; if there are more, the middle is collapsed into
        // a `"...\t(skipping N levels)"` marker. Without this, a stack-
        // overflow traceback would balloon to tens of megabytes (errors.lua's
        // stack-overflow test ran string.gmatch over the resulting buffer).
        const LEVELS1: usize = 10;
        const LEVELS2: usize = 11;
        // Collect visible frames in top-down order (deepest first). Both Lua
        // activations and pcall/xpcall continuations (which stand in for a
        // C-level pcall on the stack) are visible; PUC's traceback enumerates
        // both via lua_getstack. db.lua :715 expects "pcall" to appear.
        #[derive(Clone, Copy)]
        enum VFrame {
            Lua(usize),
            CPcall,
            CXpcall,
        }
        let mut visible: Vec<VFrame> = Vec::new();
        for (fi, cf) in self.frames.iter().enumerate().rev() {
            match cf {
                CallFrame::Lua(_) => visible.push(VFrame::Lua(fi)),
                CallFrame::Cont(nc) => match nc.kind {
                    ContKind::Pcall => visible.push(VFrame::CPcall),
                    ContKind::Xpcall { .. } => visible.push(VFrame::CXpcall),
                    _ => {}
                },
            }
        }
        // PUC `luaL_traceback` starts enumerating at the given `level` (in
        // terms of L1's CallInfo chain). For the running-thread case the C
        // frame for debug.traceback itself is level 0 and luna's `visible`
        // doesn't include it — so level=1 (PUC default) means "emit from the
        // innermost Lua frame" (visible[0..]); level=k skips k-1 frames from
        // the top. level<=0 emits nothing extra here (d_traceback handles the
        // "[C]: in function 'traceback'" prefix for level==0 separately).
        let skip = (level - 1).max(0) as usize;
        if skip >= visible.len() {
            return Vec::new();
        }
        let visible = &visible[skip..];
        let total = visible.len();
        let mut out = Vec::new();
        let emit_frame = |out: &mut Vec<u8>, v: VFrame, this: &Vm| match v {
            VFrame::Lua(fi) => {
                let f = this.frames[fi].lua().expect("Lua frame");
                let proto = f.closure.proto;
                let src = chunk_display_name(proto.source.as_ptr());
                let pc = (f.pc as usize)
                    .saturating_sub(1)
                    .min(proto.lines.len().saturating_sub(1));
                let line = proto.lines.get(pc).copied().unwrap_or(0);
                out.extend_from_slice(b"\n\t");
                out.extend_from_slice(src);
                out.extend_from_slice(format!(":{line}: in ").as_bytes());
                if let Some((namewhat, name)) = this.frame_name(fi) {
                    out.extend_from_slice(format!("{namewhat} '{name}'").as_bytes());
                } else if proto.line_defined == 0 {
                    out.extend_from_slice(b"main chunk");
                } else {
                    out.extend_from_slice(
                        format!(
                            "function <{}:{}>",
                            String::from_utf8_lossy(src),
                            proto.line_defined
                        )
                        .as_bytes(),
                    );
                }
            }
            VFrame::CPcall => out.extend_from_slice(b"\n\t[C]: in function 'pcall'"),
            VFrame::CXpcall => out.extend_from_slice(b"\n\t[C]: in function 'xpcall'"),
        };
        if total <= LEVELS1 + LEVELS2 {
            for &v in visible {
                emit_frame(&mut out, v, self);
            }
        } else {
            for &v in &visible[..LEVELS1] {
                emit_frame(&mut out, v, self);
            }
            let dropped = total - LEVELS1 - LEVELS2;
            out.extend_from_slice(format!("\n\t...\t(skipping {dropped} levels)").as_bytes());
            for &v in &visible[total - LEVELS2..] {
                emit_frame(&mut out, v, self);
            }
        }
        out
    }
}
