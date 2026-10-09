//! Function objects: compiled prototypes, Lua closures, upvalues.

use crate::runtime::fnv::FnvHash128;
use crate::runtime::heap::{Gc, GcHeader};
use crate::runtime::mem::LSlice;
use crate::runtime::string::LuaStr;
use crate::runtime::value::Value;
use crate::vm::isa::Inst;

pub use crate::runtime::call_frame::{
    AfterClose, CallFrame, CloseCont, ContKind, Frame, FrameTm, HostCont, MetaAction, MetaCont,
    NativeCont,
};
pub use crate::runtime::debug_info::{DebugName, LocVar, UpvalDesc};
pub use crate::runtime::upvalue::{UpvalState, Upvalue};

/// An unused slot of [`Proto::trace_heads`].
#[doc(hidden)]
pub const TRACE_HEADS_NONE: u32 = u32::MAX;
/// [`Proto::trace_heads`] once more traces can be entered than it holds.
#[doc(hidden)]
pub const TRACE_HEADS_MANY: u32 = u32::MAX - 1;
/// Slots in [`Proto::trace_heads`].
#[doc(hidden)]
pub const TRACE_HEADS_CAP: usize = 4;

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
    /// PUC 5.2 / 5.3 closure cache (`Proto.cache`): the last LClosure built from
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
    /// JIT cache slot. `Untried` on Proto creation; the first
    /// `Vm::call_value` on a closure whose body fits the JIT whitelist
    /// flips it to `Compiled(fn ptr)` and the `JitHandle` that backs
    /// the mmap is parked on the `Vm.jit_handles` Vec for the Vm's
    /// lifetime. `Failed` records the whitelist miss so subsequent
    /// calls skip the compile attempt.
    pub jit: std::cell::Cell<JitProtoState>,
    /// Code a backend is still compiling for this function in the
    /// background: the entry it stores once ready (0 until then), with the
    /// ABI of `jit`'s, which the next call from the interpreter puts in
    /// place of `jit`'s entry. `None` when nothing is pending.
    pub jit_next: std::cell::Cell<Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>>,
    /// Trace JIT hot-loop detector. Incremented by `Vm::run`
    /// on each backward-jump dispatched within this Proto. Once the
    /// counter passes `TRACE_HOT_THRESHOLD`, the next visit to the
    /// backward-jump target promotes that PC to a trace head and
    /// begins recording. `Cell<u32>` matches the interp's
    /// single-threaded dispatch and pays no atomic cost. Cap at
    /// `u32::MAX / 2` to leave headroom above the threshold.
    pub trace_hot_count: std::cell::Cell<u32>,
    /// Trace-on-call counter. Incremented by `begin_call` on
    /// every Lua-callee push into this Proto. Once it passes
    /// `CALL_HOT_THRESHOLD`, the next call into this Proto promotes
    /// `pc=0` to a trace head and begins recording. Lets the trace
    /// JIT cover self-recursive functions whose body holds no
    /// negative `Op::Jmp` (`fib`, recursive `make`/`check` in
    /// `binary_trees`), where the back-edge counter never triggers.
    pub call_hot_count: std::cell::Cell<u32>,
    /// Count of "partial-coverage" discards on
    /// this Proto's call-triggered recordings. Each discard is a
    /// new opportunity for the recorder to record a different
    /// (hopefully longer) trace at a deeper recursion point; the
    /// trigger condition re-uses `c >= THRESHOLD &&
    /// !already_cached` so the next call retries. Without
    /// a cap, pathologically-branchy workloads like binary_trees
    /// (`make` body contains 2 nested self-recursive calls)
    /// produce a 1500+ discard storm — the recorder never
    /// captures a covered trace because every base / shallow-
    /// depth entry caught yields a partial path. The cap
    /// bounds the storm: after `MAX_DISCARDS = 5` discards, the
    /// next close skips the coverage check and compiles + caches
    /// whatever shape it has (length gate will likely refuse
    /// dispatch but at least the trigger stops firing).
    pub trace_discard_count: std::cell::Cell<u32>,
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
    pub trace_gave_up: std::cell::Cell<bool>,
    /// Trace heads whose recordings failed to compile. The hot counters
    /// are not reset after a recording, so without this every later call
    /// or back-edge would record and compile the same failing trace again.
    pub(crate) trace_compile_failures: crate::jit::send_compat::TRefLock<Vec<HeadFailures>>,
    /// The prototypes of other functions this prototype's traces inlined.
    /// A trace checks a callee against them by address, so the collector
    /// keeps them alive while this prototype lives (`Proto::trace`).
    pub(crate) inlined_protos: std::cell::RefCell<Vec<Gc<Proto>>>,
    /// Compiled trace cache for this Proto. A successful
    /// `compile_trace(record)` parks its `CompiledTrace` here;
    /// `Vm::run`'s trace dispatcher iterates this on each
    /// back-edge target visit. `RefCell` because compile is invoked
    /// from inside `Vm::run` and may need to push while another op
    /// is mid-dispatch in the same Proto.
    pub traces: crate::jit::send_compat::TRefLock<
        Vec<crate::jit::send_compat::TArc<crate::jit::trace::CompiledTrace>>,
    >,
    /// Whether `traces` holds one the dispatcher may enter at its head
    /// (dispatchable, or carrying a down-recursion link). The
    /// interpreter checks this before scanning `traces` on every
    /// instruction. Traces are never removed, so it only goes from
    /// `false` to `true`.
    #[doc(hidden)]
    pub has_dispatchable_trace: std::cell::Cell<bool>,
    /// Head pcs of the traces that set `has_dispatchable_trace`, so the
    /// interpreter looks traces up only where one can start: up to
    /// [`TRACE_HEADS_CAP`], the rest [`TRACE_HEADS_NONE`]; [`TRACE_HEADS_MANY`]
    /// in all once there are more.
    #[doc(hidden)]
    pub trace_heads: std::cell::Cell<[u32; TRACE_HEADS_CAP]>,
    /// Whether the call trigger is done with this Proto's entry (`pc = 0`):
    /// a trace is cached there or recording it was abandoned. Set once,
    /// it spares every later call the scan of `traces`.
    #[doc(hidden)]
    pub trace_call_head_settled: std::cell::Cell<bool>,
}

/// Per-Proto JIT cache state. Copy so it fits a plain
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
        /// Per-arg Float bit. Bit `i = 1` ↔ arg slot `i`
        /// is f64 (passed as i64 bit-pattern across the ABI, bitcast
        /// inside the JIT). Bit `i = 0` ↔ Int. Bits ≥ MAX_JIT_ARITY
        /// are zero.
        arg_float_mask: u8,
        /// Per-arg Table bit. Bit `i = 1` ↔ arg slot `i`
        /// is `Gc<Table>` raw ptr (passed as the i64 pointer value
        /// directly, since `Gc<Table>` is `NonNull<Table>` =
        /// pointer-shaped). Mutually exclusive with `arg_float_mask`
        /// for the same bit. Required so `try_jit_call_op`'s arg
        /// marshalling can accept `Value::Table(t)` and pack
        /// `t.as_ptr() as i64`; without it a Table arg would fall
        /// into the dispatcher's default-deny match arm and the
        /// callee couldn't be reached via JIT.
        arg_table_mask: u8,
        /// True iff the chunk's `Return1` value is f64.
        /// Dispatcher wraps `r` as `Value::Float(f64::from_bits(r))`
        /// vs `Value::Int(r)` accordingly. Meaningful only when
        /// `returns_one == true`.
        ret_is_float: bool,
        /// True iff the chunk's `Return1` value is a
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
    /// Stable 128-bit hash over a
    /// Proto's identity-defining bytes. Two `Proto`s whose Lua source +
    /// dialect compile to the same bytecode hash to the same digest;
    /// distinct sources hash distinct. The digest is stable across
    /// `dump` / `undump` round-trips and across separate process runs,
    /// so an AOT pipeline (which fingerprints protos at compile time)
    /// and the deploy `Vm` (which fingerprints the same protos after
    /// undumping the embedded bytecode) agree on which `(Proto, pc)`
    /// site a precompiled trace targets.
    ///
    /// # What's fed into the hash
    ///
    /// - `code`: the raw u32 packed words, in order.
    /// - `consts`: per entry, a one-byte discriminant + payload bytes
    ///   (Int/Float as raw 8-byte LE; Str as `[len_u32_le | bytes]`;
    ///   Nil/Bool as discriminant alone). Heap-pointer variants in
    ///   `Value` (Table / Closure / Native / Coro / Userdata /
    ///   LightUserdata) never appear in a Proto's constant table —
    ///   constants are restricted to nil / bool / number / string by
    ///   the Lua compiler — so a `debug_assert!` catches the contract
    ///   if a future refactor changes that.
    /// - `upvals`: per descriptor, `in_stack` byte + `index` byte +
    ///   `read_only` byte + name bytes (length-prefixed u32 LE).
    /// - `num_params`, `is_vararg`, `max_stack`: single-byte each.
    ///
    /// # What's NOT fed in
    ///
    /// - Nested `protos`: each nested Proto has its own `stable_hash`;
    ///   parent identity is determined by its own immediate bytes only.
    ///   Callers that need a "whole tree" identity should hash the
    ///   roots they care about.
    /// - `lines`, `locvars`, `source`, `line_defined`,
    ///   `last_line_defined`: debug metadata. A `.lua` source edited
    ///   to add a comment shouldn't invalidate AOT traces — bytecode
    ///   is the identity, not the editor cursor.
    /// - JIT cache fields (`jit`, `traces`, `trace_hot_count`, …),
    ///   `cache`, `has_vararg_table_pseudo`, `has_compat_vararg_arg`,
    ///   `env_upval_idx`: runtime-only state derived from the
    ///   load-bearing fields above.
    ///
    /// # Algorithm
    ///
    /// Hand-rolled FNV-1a-128 (no third-party deps — `luna-core` 0-dep
    /// contract is hard). The standard 128-bit constants:
    ///
    /// - offset basis = `0x6c62272e07bb014262b821756295c58d`
    /// - prime        = `0x0000000001000000000000000000013b`
    ///
    /// Collision resistance suffices for AOT proto ID — collisions
    /// would manifest as a precompiled trace dispatched against the
    /// wrong Proto, but the dispatcher's existing guards (entry_tags
    /// match, head_pc match, register types match) would deopt to
    /// interp on a mismatch rather than corrupt state.
    pub fn stable_hash(&self) -> [u8; 16] {
        let mut h = FnvHash128::new();
        // 1. Bytecode words — `Inst` is `repr(transparent)` over u32;
        //    feed the raw little-endian bytes so the hash matches
        //    cross-platform (luna only targets little-endian platforms
        //    today, but the explicit LE serialization future-proofs).
        for inst in self.code.iter() {
            h.update(&inst.0.to_le_bytes());
        }
        // 2. Constants — discriminant + payload. Keep the discriminant
        //    byte values stable: bumping the `Value` enum order would
        //    invalidate AOT cache files, but that's the same constraint
        //    as `Value::tag_byte` already imposes.
        for c in self.consts.iter() {
            match c {
                Value::Nil => h.update(&[0u8]),
                Value::Bool(b) => {
                    h.update(&[1u8, *b as u8]);
                }
                Value::Int(i) => {
                    h.update(&[2u8]);
                    h.update(&i.to_le_bytes());
                }
                Value::Float(f) => {
                    // Hash the bit pattern so +0.0 / -0.0 don't
                    // collide and NaNs are stable across runs.
                    h.update(&[3u8]);
                    h.update(&f.to_bits().to_le_bytes());
                }
                Value::Str(s) => {
                    h.update(&[4u8]);
                    let bytes = s.as_bytes();
                    h.update(&(bytes.len() as u32).to_le_bytes());
                    h.update(bytes);
                }
                // Heap-pointer constants are not produced by the Lua
                // compiler. A debug_assert keeps the contract honest
                // without paying a runtime cost in release.
                Value::Table(_)
                | Value::Closure(_)
                | Value::Native(_)
                | Value::Coro(_)
                | Value::Userdata(_)
                | Value::LightUserdata(_) => {
                    debug_assert!(
                        false,
                        "Proto::stable_hash: unexpected heap-pointer constant \
                         (kind={}); luna's compiler only emits nil/bool/number/string \
                         constants",
                        c.type_name()
                    );
                    // Fall-through default: treat as a NUL byte. Won't
                    // happen in practice (compiler invariant), so the
                    // exact behaviour doesn't matter.
                    h.update(&[255u8]);
                }
            }
        }
        // 3. Upvalue descriptors — `name` bytes affect debug.getinfo
        //    only, but they're cheap and bytecode-equivalent compiles
        //    always produce equal names, so include them.
        for u in self.upvals.iter() {
            h.update(&[u.in_stack as u8, u.index, u.read_only as u8]);
            let name_bytes = u.name.as_bytes();
            h.update(&(name_bytes.len() as u32).to_le_bytes());
            h.update(name_bytes);
        }
        // 4. Signature bytes.
        h.update(&[self.num_params, self.is_vararg as u8, self.max_stack]);
        h.finish()
    }
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
    pub(crate) kind: crate::vm::exec::native_call::NativeKind,
    /// Which library function this is, given when it was created.
    pub builtin: crate::runtime::Builtin,
}

#[path = "function_trace.rs"]
mod trace;
#[path = "function_upvals.rs"]
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
