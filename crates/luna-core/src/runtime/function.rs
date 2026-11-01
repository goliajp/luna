//! Function objects: compiled prototypes, Lua closures, upvalues.

use crate::runtime::heap::{Gc, GcHeader};
use crate::runtime::mem::LSlice;
use crate::runtime::string::LuaStr;
use crate::runtime::value::Value;
use crate::vm::isa::Inst;

pub use crate::runtime::call_frame::{
    AfterClose, CallFrame, CloseCont, ContKind, Frame, FrameTm, HostCont, MetaAction, MetaCont,
    NativeCont,
};
pub(crate) use crate::runtime::call_frame::{DeadStack, NativeAct};
pub use crate::runtime::debug_info::{DebugName, LocVar, UpvalDesc};
pub use crate::runtime::upvalue::{UpvalState, Upvalue};

/// An unused slot of [`Proto::trace_heads`].
pub(crate) const TRACE_HEADS_NONE: u32 = u32::MAX;
/// [`Proto::trace_heads`] once more traces can be entered than it holds.
pub(crate) const TRACE_HEADS_MANY: u32 = u32::MAX - 1;
/// Slots in [`Proto::trace_heads`].
pub(crate) const TRACE_HEADS_CAP: usize = 4;

/// A compiled function (PUC Proto). Immutable after compilation.
#[repr(C)]
pub struct Proto {
    pub(crate) hdr: GcHeader,
    /// Bytecode instructions, in execution order.
    pub code: LSlice<Inst>,
    /// Constant table referenced by `LoadK` / `*K` opcodes.
    pub consts: LSlice<Value>,
    /// Nested prototypes referenced by `Closure`.
    pub protos: LSlice<Gc<Proto>>,
    /// Upvalue descriptors (one per upvalue this function captures).
    pub upvals: LSlice<UpvalDesc>,
    /// Fixed parameter count.
    pub num_params: u8,
    /// Whether the function accepts `...`.
    pub is_vararg: bool,
    /// PUC `lparser.c` emits a hidden `(vararg table)` locvar for a function
    /// declared with an explicit anonymous `(...)` (and NOT for a main chunk's
    /// implicit vararg, nor for `(...t)` which becomes a named local). The
    /// compiler gives it register `num_params`, as PUC does; the flag says
    /// that this function was compiled with it (a loaded chunk has it in its
    /// locals without the flag).
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
    pub lines: LSlice<u32>,
    /// chunk name, for error messages
    pub source: Gc<LuaStr>,
    /// Source line where the function was defined.
    pub line_defined: u32,
    /// line of the function's closing `end` (PUC `lastlinedefined`); 0 for the
    /// main chunk
    pub last_line_defined: u32,
    /// local-variable debug records (name + live pc range)
    pub locvars: LSlice<LocVar>,
    /// PUC 5.2 / 5.3 closure cache (`Proto.cache`): the last LClosure built
    /// from this Proto, reused when OP_CLOSURE finds the same upvalues in it
    /// (`getcached`; closure.lua asserts a loop's closures over `a` and
    /// `_ENV` compare equal).
    pub(crate) cache: std::cell::Cell<Option<Gc<LuaClosure>>>,
    /// Index into `upvals` of the `_ENV` upvalue (5.1 per-function-env
    /// model needs to clone-on-closure), or `u8::MAX` for "no _ENV
    /// upval". Computed once at Proto construction so `Op::Closure`'s
    /// 5.1 path doesn't string-compare across `upvals` per closure.
    pub env_upval_idx: u8,
    /// JIT cache slot. `Untried` on Proto creation; the first
    /// `Vm::call_value` on a closure whose body fits the JIT whitelist
    /// flips it to `Compiled(fn ptr)` and the `JitHandle` that backs
    /// the mmap is parked on the `Vm.jit_handles` Vec for the Vm's
    /// lifetime. `Failed` records the whitelist miss so subsequent
    /// calls skip the compile attempt.
    pub(crate) jit: std::cell::Cell<JitProtoState>,
    /// Code a backend is still compiling for this function in the
    /// background: the entry it stores once ready (0 until then), with the
    /// ABI of `jit`'s, which the next call from the interpreter puts in
    /// place of `jit`'s entry; [`JIT_NEXT_NONE`] when it compiled none.
    /// `None` when nothing is pending.
    pub(crate) jit_next: std::cell::Cell<Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>>,
    /// Trace JIT hot-loop detector. Incremented by `Vm::run`
    /// on each backward-jump dispatched within this Proto. Once the
    /// counter passes `TRACE_HOT_THRESHOLD`, the next visit to the
    /// backward-jump target promotes that PC to a trace head and
    /// begins recording. `Cell<u32>` matches the interp's
    /// single-threaded dispatch and pays no atomic cost. Cap at
    /// `u32::MAX / 2` to leave headroom above the threshold.
    pub(crate) trace_hot_count: std::cell::Cell<u32>,
    /// Trace-on-call counter. Incremented by `begin_call` on
    /// every Lua-callee push into this Proto. Once it passes
    /// `CALL_HOT_THRESHOLD`, the next call into this Proto promotes
    /// `pc=0` to a trace head and begins recording. Lets the trace
    /// JIT cover self-recursive functions whose body holds no
    /// negative `Op::Jmp` (`fib`, recursive `make`/`check` in
    /// `binary_trees`), where the back-edge counter never triggers.
    pub(crate) call_hot_count: std::cell::Cell<u32>,
    /// Count of "partial-coverage" discards on this Proto's call-triggered
    /// recordings, each a chance to record a longer trace at a deeper
    /// recursion point (the trigger lets the next call retry). Without
    /// a cap, pathologically-branchy workloads like binary_trees
    /// (`make` body contains 2 nested self-recursive calls)
    /// produce a 1500+ discard storm — the recorder never
    /// captures a covered trace because every base / shallow-
    /// depth entry caught yields a partial path. The cap
    /// bounds the storm: after `MAX_DISCARDS = 5` discards, the
    /// next close skips the coverage check and compiles + caches
    /// whatever shape it has (length gate will likely refuse
    /// dispatch but at least the trigger stops firing).
    pub(crate) trace_discard_count: std::cell::Cell<u32>,
    /// Once the discard cap forces a compile on
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
    pub(crate) trace_gave_up: std::cell::Cell<bool>,
    /// Trace heads whose recordings failed to compile. The hot counters
    /// are not reset after a recording, so without this every later call
    /// or back-edge would record and compile the same failing trace again.
    pub(crate) trace_compile_failures: crate::jit::send_compat::TRefLock<Vec<HeadFailures>>,
    /// The prototypes of other functions this prototype's traces inlined.
    /// A trace checks a callee against them by address, so the collector
    /// keeps them alive while this prototype lives (`Proto::trace`).
    pub(crate) inlined_protos: std::cell::RefCell<Vec<Gc<Proto>>>,
    /// The strings PUC's prototype keeps for its local variable and upvalue
    /// names (luna's own are `DebugName`s), kept alive as long as it is.
    pub(crate) puc_names: std::cell::RefCell<Vec<Gc<LuaStr>>>,
    /// Compiled trace cache for this Proto. A successful
    /// `compile_trace(record)` parks its `CompiledTrace` here;
    /// `Vm::run`'s trace dispatcher iterates this on each
    /// back-edge target visit. `RefCell` because compile is invoked
    /// from inside `Vm::run` and may need to push while another op
    /// is mid-dispatch in the same Proto.
    pub(crate) traces: crate::jit::send_compat::TRefLock<
        Vec<crate::jit::send_compat::TArc<crate::jit::trace::CompiledTrace>>,
    >,
    /// Whether `traces` holds one the dispatcher may enter at its head
    /// (dispatchable, or carrying a down-recursion link). The
    /// interpreter checks this before scanning `traces` on every
    /// instruction. Traces are never removed, so it only goes from
    /// `false` to `true`.
    pub(crate) has_dispatchable_trace: std::cell::Cell<bool>,
    /// Head pcs of the traces that set `has_dispatchable_trace`, so the
    /// interpreter looks traces up only where one can start: up to
    /// `TRACE_HEADS_CAP`, the rest `TRACE_HEADS_NONE`; `TRACE_HEADS_MANY`
    /// in all once there are more.
    pub(crate) trace_heads: std::cell::Cell<[u32; TRACE_HEADS_CAP]>,
    /// Whether the call trigger is done with this Proto's entry (`pc = 0`):
    /// a trace is cached there or recording it was abandoned. Set once,
    /// it spares every later call the scan of `traces`.
    pub(crate) trace_call_head_settled: std::cell::Cell<bool>,
    /// The bytes PUC's prototype holds in each of its arrays (code,
    /// prototypes, constants, line info, absolute line info, local
    /// variables, upvalue descriptors), counted when it was made and
    /// uncounted when it is freed (they differ from this prototype's).
    pub(crate) puc_arrays: std::cell::Cell<[u32; 7]>,
    /// What compiled code depends on, made the first time it is asked for.
    pub(crate) content: std::cell::OnceCell<crate::jit::trace_types::content::ProtoContent>,
}

/// Closures with `≤ INLINE_UPVALS_N` upvalues skip the
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
    /// `proto.code` and `proto.consts`, which never change after the proto
    /// is built: the interpreter takes up a frame from its closure without
    /// going through the proto.
    pub(crate) code: *const crate::vm::isa::Inst,
    pub(crate) consts: *const Value,
    /// Single source of truth for "where are the upvals?". Points to
    /// either `inline_storage` (when `upvals_len <= INLINE_UPVALS_N`)
    /// or a leaked `Box<[Gc<Upvalue>]>` of `upvals_len` this closure owns
    /// (otherwise; freed by `Drop`). Set up by
    /// `Heap::new_closure*` after the LuaClosure reaches its stable
    /// heap address.
    pub(crate) upvals_ptr: *mut Gc<Upvalue>,
    pub(crate) upvals_len: u32,
    /// Inline storage for small closures. Only the first
    /// `upvals_len.min(INLINE_UPVALS_N)` slots are initialised.
    /// `Gc<Upvalue>` is `Copy` so no explicit `Drop` pass is needed.
    ///
    /// `UnsafeCell` for the same reason as `Table::inline_storage`:
    /// `upvals_ptr` is a self-referential cached pointer into this
    /// field, and a `&mut self` function-entry retag would otherwise
    /// invalidate it under Stacked Borrows (Miri reports it). All
    /// access goes through `upvals_ptr` / `.get()`.
    pub(crate) inline_storage:
        std::cell::UnsafeCell<[std::mem::MaybeUninit<Gc<Upvalue>>; INLINE_UPVALS_N]>,
}

// SAFETY: `upvals_ptr` always refers to memory the same LuaClosure
// owns (its own inline_storage or its overflow allocation). The closure is
// heap-allocated and never moves post-adoption.
unsafe impl Send for LuaClosure {}
// SAFETY: as for `Send`
unsafe impl Sync for LuaClosure {}

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
    pub upvals: crate::runtime::mem::LSlice<Value>,
    /// Marker bit for async natives: `f` is then an
    /// `crate::vm::async_drive::AsyncNativeFn` (same pointer width,
    /// transmuted at the call site), which the native-call path drives
    /// through the cooperative-yield mechanism instead of calling it.
    pub is_async: bool,
    /// Which natives the call path runs itself, from `builtin`; a call
    /// tests this one byte.
    pub(crate) kind: crate::runtime::call_frame::NativeKind,
    /// Which library function this is, given when it was created.
    pub builtin: crate::runtime::Builtin,
}

mod hash;
mod hot;
pub(crate) mod jit_state;
pub(crate) use jit_state::{JIT_NEXT_NONE, JitProtoState};
mod trace;
mod upvals;

/// The failed recordings of one trace head.
pub(crate) struct HeadFailures {
    pub(crate) head_pc: u32,
    /// Failures, weighted (see `vm::exec::trace_cache`).
    pub(crate) n: u8,
    /// When the last recording could never have been entered: the
    /// registers that held, on entry, a value no trace is entered with.
    /// While all of them still do, a new recording would end the same way.
    pub(crate) stuck: Vec<u16>,
}
