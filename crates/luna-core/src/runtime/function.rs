//! Function objects: compiled prototypes, Lua closures, upvalues.

use crate::runtime::heap::{Gc, GcHeader, Marker};
use crate::runtime::string::LuaStr;
use crate::runtime::value::Value;
use crate::vm::isa::Inst;

/// An activation record on a thread's call stack. Pure data (closure handle +
/// stack offsets), so it lives in `runtime` where the GC can trace a suspended
/// coroutine's frames.
#[derive(Clone, Copy)]
pub struct Frame {
    /// Currently executing closure.
    pub closure: Gc<LuaClosure>,
    /// stack index of register 0
    pub base: u32,
    /// Program counter (index into `closure.proto.code`).
    pub pc: u32,
    /// stack slot of the function (results land here)
    pub func_slot: u32,
    /// number of extra (vararg) arguments, living on the stack just below `base`
    /// at `func_slot+1 .. func_slot+1+n_varargs` (PUC `CallInfo.u.l.nextraargs`).
    /// `OP_VARARG`/`OP_VARGIDX` read them there; a named vararg only materializes
    /// a heap table when it is written / escapes / is `_ENV`.
    pub n_varargs: u32,
    /// results expected by the caller (-1 = all)
    pub nresults: i32,
    /// pc the line hook last observed in this frame (PUC CallInfo `oldpc`);
    /// `u32::MAX` on a fresh frame so its first instruction fires a line event
    pub hook_oldpc: u32,
    /// true if this Lua frame was entered across a C boundary (call_value: a
    /// metamethod, pcall, __close handler, or a coroutine body). Debug level
    /// traversal (`debug.getinfo`/traceback) inserts a synthetic C frame below it.
    pub from_c: bool,
    /// the metamethod event this frame is handling (e.g. "close" for a `__close`
    /// handler call), so `debug.traceback`/getinfo can name it "metamethod
    /// 'close'" (PUC `CallInfo.u.l.tm`/`luaG_funcnamefromtm`).
    pub tm: Option<&'static str>,
    /// true when this frame is the hook function itself (PUC sets
    /// `CIST_HOOKED`). `debug.getinfo(1).namewhat` returns `"hook"` for it.
    pub is_hook: bool,
    /// PUC `ci->u.l.tailcalls` — how many tail calls have collapsed into
    /// this activation slot. Each `OP_TailCall` chain adds one. 5.1
    /// `lua_getstack` reports a synthetic `CIST_TAIL` level per count
    /// (so a deeply tail-recursive function shows `tailcalls` extra
    /// levels between itself and its real caller — 5.1 db.lua :372 walks
    /// `getinfo(2..lim)` and expects each to be `"tail"`). The 5.2+
    /// `istailcall` boolean is `tailcalls > 0`.
    pub tailcalls: u32,
}

/// An entry on a thread's call stack: either a Lua activation record or a
/// continuation frame standing in for a *yieldable native* (pcall/xpcall).
///
/// A `Cont` sits just below the call it protects. When that call returns,
/// yields-to-completion, or errors, the interpreter consumes the `Cont` to wrap
/// the outcome — the analogue of PUC `lua_pcallk`'s continuation `k`. Keeping it
/// on the same stack as Lua frames means a `coroutine.yield` crossing it is
/// preserved and restored automatically with the thread's saved context.
#[derive(Clone, Copy)]
pub enum CallFrame {
    /// A Lua activation record.
    Lua(
        /// The activation record.
        Frame,
    ),
    /// A continuation guarding a yieldable native call (pcall / xpcall /
    /// metamethod / `__close` / `__pairs`).
    Cont(
        /// The continuation record.
        NativeCont,
    ),
}

impl CallFrame {
    /// Borrow the inner Lua frame if this is a `Lua` variant.
    #[inline]
    pub fn lua(&self) -> Option<&Frame> {
        match self {
            CallFrame::Lua(f) => Some(f),
            CallFrame::Cont(_) => None,
        }
    }

    /// Mutably borrow the inner Lua frame if this is a `Lua` variant.
    #[inline]
    pub fn lua_mut(&mut self) -> Option<&mut Frame> {
        match self {
            CallFrame::Lua(f) => Some(f),
            CallFrame::Cont(_) => None,
        }
    }
}

/// A continuation frame for `pcall`/`xpcall`: where its wrapped result lands and
/// how to wrap it. Lives on the call stack below the protected call (see
/// [`CallFrame`]).
#[derive(Clone, Copy)]
pub struct NativeCont {
    /// What kind of protection this continuation represents.
    pub kind: ContKind,
    /// the protecting native's own stack slot — the wrapped status + values
    /// (`true, …` / `false, msg`) land here
    pub func_slot: u32,
    /// results the caller of pcall/xpcall expects (-1 = all)
    pub nresults: i32,
}

/// Continuation kind for yieldable native dispatch.
#[derive(Clone, Copy)]
pub enum ContKind {
    /// `pcall(f, ...)` — wraps the result as `(true, ...)` / `(false, msg)`.
    Pcall,
    /// xpcall: the message handler to run if the protected call errors
    Xpcall {
        /// Message handler function invoked on error.
        handler: Value,
    },
    /// a yieldable metamethod call triggered by a VM instruction (PUC's
    /// `luaV_finishOp`): on the metamethod's return the interrupted instruction
    /// is completed per `MetaCont`. A `coroutine.yield` inside the metamethod is
    /// preserved on the thread's frame stack like any other call.
    Meta(
        /// Continuation describing how to finish the interrupted op.
        MetaCont,
    ),
    /// a yieldable `__pairs` metamethod call from `pairs()` (PUC luaB_pairs uses
    /// lua_callk): on return, its (≤4, nil-padded) results are `pairs`'s own
    /// results. A `coroutine.yield` inside `__pairs` is preserved like pcall's.
    Pairs,
    /// a yieldable `__close` handler call driven by `begin_close` (PUC's
    /// `luaF_close` + `lua_callk` continuation). On the handler's return or
    /// error, the close iteration resumes from `CloseCont`'s state and either
    /// invokes the next handler (pushing a fresh Cont::Close) or executes the
    /// recorded `AfterClose` action.
    Close(
        /// Per-iteration close state.
        CloseCont,
    ),
}

/// Per-iteration state for a chain of `__close` handlers driven through the
/// interpreter loop. When a handler is pushed onto the call stack, this rides
/// in a `Cont::Close` frame underneath it so a `coroutine.yield` from the
/// handler preserves the close iteration with the rest of the thread.
#[derive(Clone, Copy)]
pub struct CloseCont {
    /// the close threshold: keep closing tbc slots ≥ from until exhausted
    pub from: u32,
    /// the error object threaded through subsequent handlers, if any
    pub pending: Option<Value>,
    /// what to do once every slot ≥ from is closed
    pub after: AfterClose,
}

/// What to run once `begin_close` has drained every tbc slot.
#[derive(Clone, Copy)]
pub enum AfterClose {
    /// `OP_Close` (block-end close): nothing else; next instruction continues.
    Block,
    /// `OP_Return*`: pop the Lua frame whose `OP_Return` triggered the close
    /// and deliver `nret` results from `[abs_a, abs_a + nret)` to the frame's
    /// `func_slot`. `from_native` mirrors the original op's hook flag.
    Return {
        /// Absolute stack index of the first return value.
        abs_a: u32,
        /// Number of return values.
        nret: u32,
        /// Mirrors the original op's hook-fired flag.
        from_native: bool,
    },
    /// Error unwind: the close runs while unwinding a Lua frame. When every
    /// handler is done, pop the deferred Lua frame, truncate to `func_slot`,
    /// and re-raise — preferring a handler-raised error over `err` (PUC
    /// luaF_close).
    ResumeUnwind {
        /// Slot to truncate the value stack to before re-raising.
        func_slot: u32,
        /// Original error value to re-raise (or replaced by a handler raise).
        err: Value,
    },
}

/// How to complete a VM instruction once its metamethod returns.
#[derive(Clone, Copy)]
pub struct MetaCont {
    /// What to do with the metamethod's return value.
    pub action: MetaAction,
    /// the interrupted frame's `top` to restore after the metamethod returns
    pub saved_top: u32,
}

/// Per-op finishing action for a yielded metamethod call.
#[derive(Clone, Copy)]
pub enum MetaAction {
    /// arithmetic / index / unary / length: store the single result at `dst`
    Store {
        /// Destination register receiving the metamethod's first result.
        dst: u32,
    },
    /// `__newindex`: the metamethod has no result to keep
    Discard,
    /// comparison (`__eq`/`__lt`/`__le`): the truthiness of the result feeds the
    /// conditional skip — the following JMP runs iff `result.truthy() == k`.
    /// `negate=true` flips the truthiness first, for the ≤5.3 `__le` →
    /// `not __lt(b, a)` synthesis path where the metamethod is `__lt` but
    /// the operator was `<=`.
    Compare {
        /// Sense of the conditional skip the comparison op was emitted for.
        k: bool,
        /// True when the 5.3 `__le → not __lt(b,a)` synthesis is in effect.
        negate: bool,
    },
    /// `__concat`: store the result at `dst`, set `top = dst + 1`, then continue
    /// folding the operands still at `[base_a .. top)` (PUC finishOp re-runs).
    Concat {
        /// Destination register for the metamethod's result.
        dst: u32,
        /// First operand register of the original concat span.
        base_a: u32,
    },
}

/// Where a closure's upvalue is captured from, relative to the *enclosing*
/// function (PUC Upvaldesc).
#[derive(Clone, Debug)]
pub struct UpvalDesc {
    /// captured from the enclosing frame's registers (true) or from the
    /// enclosing closure's own upvalues (false)
    pub in_stack: bool,
    /// Index in the enclosing frame's register file (when `in_stack`) or
    /// in the enclosing closure's upvalue array (otherwise).
    pub index: u8,
    /// variable name, for error messages and debug info
    pub name: Box<str>,
    /// the captured variable is `<const>` (5.5): assignment through this
    /// upvalue is a compile-time error
    pub read_only: bool,
}

/// Debug record for a local variable: its name and the pc range over which it
/// occupies register `reg`. Used to name registers in error messages and
/// debug.getinfo (PUC LocVar).
#[derive(Clone, Debug)]
pub struct LocVar {
    /// Local-variable name.
    pub name: Box<str>,
    /// Register holding the variable while in scope.
    pub reg: u32,
    /// First pc where the variable is live.
    pub start_pc: u32,
    /// Pc one past the last where the variable is live.
    pub end_pc: u32,
}

/// A compiled function (PUC Proto). Immutable after compilation.
#[repr(C)]
pub struct Proto {
    pub(crate) hdr: GcHeader,
    /// Bytecode instructions, in execution order.
    pub code: Box<[Inst]>,
    /// Constant table referenced by `LoadK` / `*K` opcodes.
    pub consts: Box<[Value]>,
    /// Nested prototypes referenced by `Closure`.
    pub protos: Box<[Gc<Proto>]>,
    /// Upvalue descriptors (one per upvalue this function captures).
    pub upvals: Box<[UpvalDesc]>,
    /// Fixed parameter count.
    pub num_params: u8,
    /// Whether the function accepts `...`.
    pub is_vararg: bool,
    /// PUC `lparser.c` emits a hidden `(vararg table)` locvar for a function
    /// declared with an explicit anonymous `(...)` (and NOT for a main chunk's
    /// implicit vararg, nor for `(...t)` which becomes a named local). When
    /// true, `debug.getlocal` exposes the pseudo at `num_params + 1`.
    pub has_vararg_table_pseudo: bool,
    /// PUC 5.1 `LUAI_COMPAT_VARARG`: the function declared `...` and so gets a
    /// hidden local named `arg` at `num_params` populated at entry with the
    /// extra args as `{n = count, [1] = e1, [2] = e2, …}`. The slot keeps the
    /// shape across resumes; user code can reassign it. 5.1 db.lua :279 reads
    /// `arg.n` from inside a `line` hook walking `debug.getlocal(2, i)`.
    pub has_compat_vararg_arg: bool,
    /// registers needed by a frame of this function
    pub max_stack: u8,
    /// line of each instruction (same length as `code`)
    pub lines: Box<[u32]>,
    /// chunk name, for error messages
    pub source: Gc<LuaStr>,
    /// Source line where the function was defined.
    pub line_defined: u32,
    /// line of the function's closing `end` (PUC `lastlinedefined`); 0 for the
    /// main chunk
    pub last_line_defined: u32,
    /// local-variable debug records (name + live pc range)
    pub locvars: Box<[LocVar]>,
    /// PUC 5.2+ closure cache (`Proto.cache`): the last LClosure built from
    /// this Proto. When OP_CLOSURE fires, the VM compares each candidate
    /// upvalue to the cached closure's same-slot upvalue (`getcached`); on a
    /// full match the cached closure is reused, so two `function() ... end`
    /// literals reached from the same source compile but with identical
    /// upvalue bindings compare equal. closure.lua's `for i=1,5 do
    /// a[i]=function(x) return x+a+_ENV end end` asserts that subsequent
    /// iterations reuse the closure; capturing `i` instead defeats the cache.
    pub cache: std::cell::Cell<Option<Gc<LuaClosure>>>,
    /// Index into `upvals` of the `_ENV` upvalue (5.1 per-function-env
    /// model needs to clone-on-closure), or `u8::MAX` for "no _ENV
    /// upval". Computed once at Proto construction so `Op::Closure`'s
    /// 5.1 path doesn't string-compare across `upvals` per closure.
    pub env_upval_idx: u8,
    /// P11-S2 — JIT cache slot. `Untried` on Proto creation; the first
    /// `Vm::call_value` on a closure whose body fits the S1 whitelist
    /// flips it to `Compiled(fn ptr)` and the `JitHandle` that backs
    /// the mmap is parked on the `Vm.jit_handles` Vec for the Vm's
    /// lifetime. `Failed` records the whitelist miss so subsequent
    /// calls skip the compile attempt.
    pub jit: std::cell::Cell<JitProtoState>,
    /// P12-S1 — trace JIT hot-loop detector. Incremented by `Vm::run`
    /// on each backward-jump dispatched within this Proto. Once the
    /// counter passes `TRACE_HOT_THRESHOLD`, the next visit to the
    /// backward-jump target promotes that PC to a trace head and
    /// begins recording (S2+). `Cell<u32>` matches the interp's
    /// single-threaded dispatch and pays no atomic cost. Cap at
    /// `u32::MAX / 2` to leave headroom above the threshold.
    pub trace_hot_count: std::cell::Cell<u32>,
    /// P12-S4 — trace-on-call counter. Incremented by `begin_call` on
    /// every Lua-callee push into this Proto. Once it passes
    /// `CALL_HOT_THRESHOLD`, the next call into this Proto promotes
    /// `pc=0` to a trace head and begins recording. Lets the trace
    /// JIT cover self-recursive functions whose body holds no
    /// negative `Op::Jmp` (`fib`, recursive `make`/`check` in
    /// `binary_trees`), where the back-edge counter never triggers.
    pub call_hot_count: std::cell::Cell<u32>,
    /// P13-S13-I — count of S13-H "partial-coverage" discards on
    /// this Proto's call-triggered recordings. Each discard is a
    /// new opportunity for the recorder to record a different
    /// (hopefully longer) trace at a deeper recursion point; the
    /// trigger condition re-uses `c >= THRESHOLD &&
    /// !already_cached` (S13-H) so the next call retries. Without
    /// a cap, pathologically-branchy workloads like binary_trees
    /// (`make` body contains 2 nested self-recursive calls)
    /// produce a 1500+ discard storm — the recorder never
    /// captures a covered trace because every base / shallow-
    /// depth entry caught yields a partial path. The S13-I cap
    /// bounds the storm: after `MAX_DISCARDS = 5` discards, the
    /// next close skips the coverage check and compiles + caches
    /// whatever shape it has (length gate will likely refuse
    /// dispatch but at least the trigger stops firing).
    pub trace_discard_count: std::cell::Cell<u32>,
    /// P13-S13-K — once the S13-I discard cap forces a compile on
    /// this Proto (the recorder gave up trying to capture a
    /// covered trace and just compiled whatever shape it had), set
    /// this flag to `true`. Both trigger gates (back-edge in
    /// `Op::Jmp` and call in `begin_call`) short-circuit on
    /// `gave_up` BEFORE doing the `proto.traces.borrow()` +
    /// linear-scan `already_cached` check. Each post-cap call into
    /// such a Proto avoids the RefCell borrow + Vec scan
    /// (`binary_trees_pattern`'s 20k make + 20k check calls per
    /// run = 40k RefCell borrows saved). The `gave_up` flag never
    /// flips back to `false` within a Vm — gave-up is permanent
    /// on the Proto, mirroring the `JitProtoState::Failed`
    /// invariant.
    pub trace_gave_up: std::cell::Cell<bool>,
    /// P12-S2 — compiled trace cache for this Proto. A successful
    /// `compile_trace(record)` (S2.B) parks its `CompiledTrace` here;
    /// `Vm::run`'s S3 dispatcher (next phase) iterates this on each
    /// back-edge target visit. `RefCell` because compile is invoked
    /// from inside `Vm::run` and may need to push while another op
    /// is mid-dispatch in the same Proto. Empty `Vec` until S2 lands.
    pub traces: std::cell::RefCell<Vec<std::rc::Rc<crate::jit::trace::CompiledTrace>>>,
}

/// P11-S2 / S2c — per-Proto JIT cache state. Copy so it fits a plain
/// `Cell` on the dispatch hot path (no `RefCell` borrow check); the
/// fn pointer's mmap is kept alive by `Vm.jit_handles`.
#[derive(Clone, Copy, Debug)]
pub enum JitProtoState {
    /// Compilation hasn't been attempted yet.
    Untried,
    /// Compilation was attempted and the body fell outside the whitelist;
    /// subsequent calls skip the attempt.
    Failed,
    /// Native code is installed and callable through the recorded entry.
    Compiled {
        /// Raw mmap'd code address. Transmute to the
        /// `unsafe extern "C" fn(i64, …) -> i64` shape matching
        /// `num_args` at the call site.
        entry: *const u8,
        /// 0..=MAX_JIT_ARITY. Picks the transmute target.
        num_args: u8,
        /// True when the Lua chunk terminates with `Return1` (single
        /// observable return value). False means the chunk only
        /// side-effects + `Return0` — host gets an empty `Vec<Value>`
        /// from `Vm::call_value`, an interpreter `Op::Call` gets
        /// zero results pushed (PUC nresults handling).
        returns_one: bool,
        /// P11-S3 — per-arg Float bit. Bit `i = 1` ↔ arg slot `i`
        /// is f64 (passed as i64 bit-pattern across the ABI, bitcast
        /// inside the JIT). Bit `i = 0` ↔ Int. Bits ≥ MAX_JIT_ARITY
        /// are zero.
        arg_float_mask: u8,
        /// P11-S5d — per-arg Table bit. Bit `i = 1` ↔ arg slot `i`
        /// is `Gc<Table>` raw ptr (passed as the i64 pointer value
        /// directly, since `Gc<Table>` is `NonNull<Table>` =
        /// pointer-shaped). Mutually exclusive with `arg_float_mask`
        /// for the same bit. Required so `try_jit_call_op`'s arg
        /// marshalling can accept `Value::Table(t)` and pack
        /// `t.as_ptr() as i64`; without it a Table arg would fall
        /// into the dispatcher's default-deny match arm and the
        /// callee couldn't be reached via JIT.
        arg_table_mask: u8,
        /// P11-S3 — true iff the chunk's `Return1` value is f64.
        /// Dispatcher wraps `r` as `Value::Float(f64::from_bits(r))`
        /// vs `Value::Int(r)` accordingly. Meaningful only when
        /// `returns_one == true`.
        ret_is_float: bool,
        /// P11-S5d — true iff the chunk's `Return1` value is a
        /// `Gc<Table>` ptr. Mutually exclusive with `ret_is_float`.
        /// Dispatcher wraps `r` as
        /// `Value::Table(Gc::from_ptr(r as *mut Table))`.
        ret_is_table: bool,
    },
}

// Cell<JitProtoState> stores raw pointers; explicit Send + Sync
// negative: keep these on a single-threaded runtime. The Vm itself
// already is !Send (Heap holds raw GcHeader pointers), so we don't
// need any auto-trait gymnastics — this comment exists so a future
// audit doesn't try to flip the trait without thinking.

impl Proto {
    pub(crate) fn trace(&self, m: &mut Marker) {
        for &k in self.consts.iter() {
            m.value(k);
        }
        for &p in self.protos.iter() {
            m.header(p.as_ptr() as *mut GcHeader);
        }
        m.header(self.source.as_ptr() as *mut GcHeader);
        // PUC `traverseproto`: the closure cache is a *weak* reference — if
        // the cached LClosure is unmarked at sweep time, clear the slot
        // instead of marking it. Queue self for the post-mark cleanup pass
        // so a closure whose only remaining live reference is the cache
        // becomes collectable (gc.lua's `__gc` finalisers inside `do ... end`
        // blocks rely on this).
        if self.cache.get().is_some() {
            m.cached_protos.push(self as *const Proto as *mut Proto);
        }
    }
}

/// P11-S5d.M — closures with `≤ INLINE_UPVALS_N` upvalues skip the
/// per-closure upvals Box. The `Op::Closure` handler builds upvals
/// into a stack array and calls `Heap::new_closure_inline(&[Gc<…>])`,
/// which writes them straight into `inline_storage` — no caller-side
/// Vec/Box. `closure_alloc`-style benchmarks create 10k single-upval
/// closures per iter; eliminating the 24-byte Vec alloc shaves ~300µs.
pub const INLINE_UPVALS_N: usize = 2;

/// A Lua closure: a `Proto` paired with its captured upvalues.
#[repr(C)]
pub struct LuaClosure {
    /// read through raw casts by the GC, not by field access
    #[allow(dead_code)]
    pub(crate) hdr: GcHeader,
    /// The compiled function body this closure binds.
    pub proto: Gc<Proto>,
    /// Single source of truth for "where are the upvals?". Points to
    /// either `inline_storage` (when `upvals_len <= INLINE_UPVALS_N`)
    /// or `overflow.as_mut_ptr()` (otherwise). Set up by
    /// `Heap::new_closure*` after the LuaClosure reaches its stable
    /// heap address.
    pub(crate) upvals_ptr: *mut Gc<Upvalue>,
    pub(crate) upvals_len: u32,
    /// Inline storage for small closures. Only the first
    /// `upvals_len.min(INLINE_UPVALS_N)` slots are initialised.
    /// `Gc<Upvalue>` is `Copy` so no explicit `Drop` pass is needed.
    pub(crate) inline_storage: [std::mem::MaybeUninit<Gc<Upvalue>>; INLINE_UPVALS_N],
    /// Overflow box for closures with `> INLINE_UPVALS_N` upvalues.
    /// Empty box (dangling, no allocation) otherwise.
    pub(crate) overflow: Box<[Gc<Upvalue>]>,
}

// SAFETY: `upvals_ptr` always refers to memory the same LuaClosure
// owns (its own inline_storage or its `overflow` Box). The closure is
// heap-allocated and never moves post-adoption.
unsafe impl Send for LuaClosure {}
unsafe impl Sync for LuaClosure {}

impl LuaClosure {
    /// View of all upvalues as a `&[Gc<Upvalue>]`. Backed by inline
    /// storage when `upvals_len <= INLINE_UPVALS_N`, else by overflow.
    #[inline(always)]
    pub fn upvals(&self) -> &[Gc<Upvalue>] {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { std::slice::from_raw_parts(self.upvals_ptr, self.upvals_len as usize) }
    }

    #[inline(always)]
    pub(crate) fn upvals_mut(&mut self) -> &mut [Gc<Upvalue>] {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { std::slice::from_raw_parts_mut(self.upvals_ptr, self.upvals_len as usize) }
    }

    /// Wire `upvals_ptr` to the active backing storage. Called by the
    /// Heap closure constructors once the LuaClosure is at its stable
    /// heap address (inline_storage's address is only valid after the
    /// Box::new move into the heap).
    pub(crate) fn init_upvals_ptr(&mut self) {
        if self.upvals_len as usize <= INLINE_UPVALS_N {
            self.upvals_ptr = self.inline_storage.as_mut_ptr() as *mut Gc<Upvalue>;
        } else {
            self.upvals_ptr = self.overflow.as_mut_ptr();
        }
    }

    pub(crate) fn trace(&self, m: &mut Marker) {
        m.header(self.proto.as_ptr() as *mut GcHeader);
        for &uv in self.upvals().iter() {
            m.header(uv.as_ptr() as *mut GcHeader);
        }
    }
}

/// A native (host) function with captured upvalues — the analogue of PUC C
/// closures. Builtins are allocated once at registration so identity is
/// stable; stateful iterators (gmatch) mutate their upvalues via `as_mut`.
#[repr(C)]
pub struct NativeClosure {
    /// read through raw casts by the GC, not by field access
    #[allow(dead_code)]
    pub(crate) hdr: GcHeader,
    /// The host function pointer this closure dispatches to.
    pub f: crate::runtime::value::NativeFn,
    /// Captured upvalues, visible inside `f` via the Vm's call API.
    pub upvals: Box<[Value]>,
    /// v1.1 B10 Stage 2 — marker bit for async natives. When `true`,
    /// `f` is actually an `crate::vm::async_drive::AsyncNativeFn`
    /// (same pointer width, transmuted at the call site) returning a
    /// `Pin<Box<dyn Future>>`. The dispatcher's native-call path checks
    /// this bit and routes through the cooperative-yield mechanism
    /// instead of invoking `f` synchronously. Default `false` (sync
    /// native) for all v1.0 / v1.1-Stage-1 construction sites.
    pub is_async: bool,
}

impl NativeClosure {
    pub(crate) fn trace(&self, m: &mut Marker) {
        for &v in self.upvals.iter() {
            m.value(v);
        }
    }
}

/// An upvalue cell. Open: refers to a live VM stack slot (the stack is a GC
/// root, so open cells trace nothing). Closed: owns the value inline.
#[repr(C)]
pub struct Upvalue {
    /// read through raw casts by the GC, not by field access
    #[allow(dead_code)]
    pub(crate) hdr: GcHeader,
    pub(crate) state: UpvalState,
}

/// Open / closed state of an upvalue cell.
#[derive(Clone, Copy)]
pub enum UpvalState {
    /// references slot `slot` of `thread`'s value stack (`None` = the main
    /// thread). The owning thread is tracked so the cell still resolves to the
    /// right stack after a coroutine swap (P05).
    Open {
        /// Stack slot of the captured local on the owning thread.
        slot: u32,
        /// Owning thread, or `None` for the main thread.
        thread: Option<Gc<crate::runtime::coroutine::Coro>>,
    },
    /// Captured value has been hoisted into the cell.
    Closed(
        /// The closed-over value.
        Value,
    ),
}

impl Upvalue {
    /// Return the upvalue's current state (open / closed).
    pub fn state(&self) -> UpvalState {
        self.state
    }

    pub(crate) fn set_closed(&mut self, v: Value) {
        self.state = UpvalState::Closed(v);
    }

    pub(crate) fn trace(&self, m: &mut Marker) {
        match self.state {
            UpvalState::Closed(v) => {
                m.value(v);
            }
            UpvalState::Open {
                thread: Some(co), ..
            } => {
                m.header(co.as_ptr() as *mut GcHeader);
            }
            UpvalState::Open { thread: None, .. } => {}
        }
    }
}
