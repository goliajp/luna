use super::*;

mod probe;
pub use probe::*;

/// Owner of one compiled trace's mmap'd code. Drop releases the
/// pages, so the handle is parked on the owning `Vm`'s
/// `storage.trace_handles` Vec, keeping the entry fn pointer
/// callable for the lifetime of that `Vm`.
///
/// Mirrors the method JIT's `JitHandle` /
/// `storage.cache_handles` pattern (`jit_backend/mod.rs`).
pub struct TraceHandle {
    // Sleeve `JITModule` in the
    // `SendJitModule` newtype so the module's `Send` claim is
    // expressed at the field type, not by an `unsafe impl Send` on
    // the outer struct. The wrapper is `repr(Rust)` newtype with
    // `Deref<Target = JITModule>` so any internal call site that
    // touched `handle._module.<method>` still resolves through Deref.
    pub(super) _module: crate::jit_backend::SendJitModule,
    pub(super) _entry_raw: *const u8,
}

impl TraceHandle {
    /// Frees the compiled trace.
    ///
    /// # Safety
    ///
    /// The trace is not running and will not be entered again.
    pub(crate) unsafe fn free(self) {
        // SAFETY: forwarded from the caller
        unsafe { self._module.free() }
    }

    /// `#[doc(hidden)]` accessor returning
    /// the parked `_module` borrowed at the `SendJitModule` newtype.
    /// Mirror of `JitHandle::__send_module`; lets
    /// `tests/it/jit_vm_scoped_rebind.rs` statically assert the
    /// field type.
    #[doc(hidden)]
    #[inline]
    pub fn __send_module(&self) -> &crate::jit_backend::SendJitModule {
        &self._module
    }
}

/// lowerer for Int arith + Move + Int-Int cmp
/// guards + Table ops + trace-truncating `Op::Call` on a
/// single-Proto trace.
///
/// Attempt to lower a closed [`TraceRecord`] to a native trace fn.
/// The fn's ABI is `fn(reg_state: *mut i64) -> i64` (see [`TraceFn`]).
/// At entry, the trace loads every register from the caller's
/// `reg_state` buffer into a cranelift `Variable`; the body emits
/// IR per op. Each `Lt / Le / Eq` op emits an `icmp` + `brif` —
/// on a runtime mismatch with the recorded comparison direction,
/// control diverts to a side-exit block that stores reg state back
/// and returns the failing PC. Each `NewTable / SetI / GetI / Len`
/// op emits a cranelift `call` to the matching `luna_jit_*` helper
/// (`Linkage::Import`, resolved via `JITBuilder::symbol`); helpers
/// short-circuit on `vm.jit.pending_err` so a metatable-bearing
/// table parks a deopt request the dispatcher can
/// detect after the trace returns. The clean-close tail stores
/// reg state back and returns `head_pc as i64`.
///
/// Returns `None` if:
/// - the record is not closed yet (open traces can't be entered
///   safely — the loop edge is the only sound entry/exit),
/// - any recorded op is outside `is_whitelisted_op`,
/// - any operand register index ≥ `head_proto.max_stack`,
/// - a `Lt / Le / Eq` is not followed by a `Jmp` at `cmp.pc + 1`,
/// - a `Jmp` is neither cmp-consumed nor at the trace's last
///   position,
/// - cranelift codegen fails.
///
/// On success, the underlying `JITModule` is parked on
/// `storage.trace_handles` so the returned `CompiledTrace.entry`
/// stays callable for the lifetime of the owning `Vm`.
///
/// **Caller contract for table ops**: before invoking the
/// returned entry, the caller (the dispatcher or a test harness)
/// must call [`crate::jit_backend::enter_jit`] to pin
/// the active Vm in the `JIT_VM` thread-local — the table helpers
/// pick that up to reach `vm.heap`. After the call, the caller
/// must inspect `vm.jit.pending_err` to decide whether a metatable
/// deopt fired; on `Some`, treat the trace's result as invalid and
/// re-run the work through the interpreter.
///
/// This is a convenience wrapper for callers that don't need to
/// pick options — it forwards to
/// [`try_compile_trace_with_options`] with [`CompileOptions::default`]
/// (one-shot, the shape unit tests assume).
pub fn try_compile_trace(
    storage: &mut dyn luna_core::__private::JitStorage,
    record: &TraceRecord,
) -> Option<CompiledTraceParts> {
    try_compile_trace_with_options(storage, record, CompileOptions::default())
}

/// Variant of [`try_compile_trace`] that takes a [`CompileOptions`]
/// — the close handler uses this with `internal_loop = true` so the
/// JIT'd trace runs in a native loop until a cmp side-exits.
///
/// thin wrapper around the backend-agnostic
/// [`lower_trace_into`] generic. Constructs a `JITModule`, finalizes
/// the compiled trace into RWX memory, patches the real entry fn ptr
/// into the returned [`CompiledTrace`], and stashes the module in
/// `storage.trace_handles` so the entry stays callable for the
/// lifetime of the owning `Vm`.
pub fn try_compile_trace_with_options(
    storage: &mut dyn luna_core::__private::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
) -> Option<CompiledTraceParts> {
    compile_trace_jit(storage, record, opts, true, false)
}

/// [`try_compile_trace_with_options`] for a record of dialect `version`.
#[doc(hidden)]
pub fn try_compile_trace_for(
    storage: &mut dyn luna_core::__private::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    version: luna_core::version::LuaVersion,
) -> Option<CompiledTraceParts> {
    let float_only = version <= luna_core::version::LuaVersion::Lua52;
    compile_trace_jit(storage, record, opts, true, float_only)
}

/// [`try_compile_trace_with_options`] for a Vm's trace cache: a trace
/// nothing can enter ([`trace_is_enterable`]) comes back without machine
/// code. Cranelift is most of a trace's compile time, and the cache
/// entry alone keeps the head from being recorded again. `entry` keeps
/// the placeholder, which nothing calls.
///
/// With `version` known, a Vm sharing its traces through an engine hands
/// the trace to it (see `super::share`).
pub(crate) fn compile_trace_for_vm(
    storage: &mut dyn luna_core::__private::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    float_only: bool,
    version: Option<luna_core::version::LuaVersion>,
) -> Option<CompiledTrace> {
    let capture = version.is_some()
        && crate::jit_backend::storage::from_storage(storage).is_ok_and(|cs| cs.engine.is_some());
    let (mut ct, cap) = compile_trace_captured(storage, record, opts, false, float_only, capture)?;
    if let (Some(version), Some(cap)) = (version, cap) {
        share::publish(storage, record, &mut ct, opts, version, cap);
    }
    // SAFETY: `entry` is the code just compiled from `record` against the
    // tags, window and cells `ct` holds (its tier source made with it), or
    // the placeholder of a trace never entered; the code stays mapped while
    // the Vm's storage lives, and no side-trace cell is set yet
    Some(unsafe { CompiledTrace::from_parts(ct) })
}

/// backend-agnostic body of the trace
/// lowerer. Generic over any `cranelift_module::Module` so the same
/// codegen pipeline drives the runtime JIT (`JITModule`,
/// [`try_compile_trace_with_options`]) and the AOT pipeline
/// (`ObjectModule` in `luna-aot`).
///
/// Returns `None` on the same bail conditions as
/// [`try_compile_trace`] (see its docstring). On success returns the
/// declared [`FuncId`] for the lowered trace alongside a
/// [`CompiledTrace`] whose `entry` field holds a private
/// `placeholder_trace_fn`; backend-specific finalize must patch the
/// real entry pointer before dispatch (the JIT wrapper does this; the
/// AOT pipeline resolves the symbol at link time and never invokes
/// `entry` directly).
// cranelift types in the signature: internal to luna crates, not covered by semver
#[doc(hidden)]
pub fn lower_trace_into<M: Module>(
    module: &mut M,
    record: &TraceRecord,
    opts: CompileOptions,
) -> Option<(FuncId, CompiledTraceParts)> {
    lower_trace_into_named(module, record, opts, None)
}

/// like [`lower_trace_into`] but
/// lets the caller (luna-aot) pick a unique exported name for the
/// trace function. Required for AOT: many traces from the same chunk
/// would otherwise collide on `"luna_jit_trace"`, and `Linkage::Local`
/// hides the symbol from the deploy-side staticlib's resolver.
///
/// `aot_fn_name = None` keeps the original behaviour (anonymous
/// `Linkage::Local` `"luna_jit_trace"`), so the JIT wrapper and the
/// existing AOT smoke tests are unaffected.
///
/// When `Some(name)`, `name` becomes the cranelift `FuncId` symbol
/// with `Linkage::Export`, surfacing in the produced `.o`'s symbol
/// table for the deploy-side `dlsym`/linker to resolve.
// cranelift types in the signature: internal to luna crates, not covered by semver
#[doc(hidden)]
pub fn lower_trace_into_named<M: Module>(
    module: &mut M,
    record: &TraceRecord,
    opts: CompileOptions,
    aot_fn_name: Option<&str>,
) -> Option<(FuncId, CompiledTraceParts)> {
    lower_trace_into_inner(module, record, opts, aot_fn_name, true, false, false)
}

/// [`lower_trace_into_named`] for a record of dialect `version`.
// cranelift types in the signature: internal to luna crates, not covered by semver
#[doc(hidden)]
pub fn lower_trace_into_named_for<M: Module>(
    module: &mut M,
    record: &TraceRecord,
    opts: CompileOptions,
    aot_fn_name: Option<&str>,
    version: luna_core::version::LuaVersion,
) -> Option<(FuncId, CompiledTraceParts)> {
    let float_only = version <= luna_core::version::LuaVersion::Lua52;
    lower_trace_into_inner(module, record, opts, aot_fn_name, true, float_only, false)
}

// SAFETY: `SendJitModule` is `Send` because luna only ever
// constructs `JITModule` with the default `SystemMemoryProvider`
// (which is `Send`). `_entry_raw: *const u8` is `!Send` by default;
// the manual `unsafe impl Send for TraceHandle` therefore stays
// load-bearing for the outer struct, but the wrapper localizes
// the JITModule-side soundness reasoning to one place.
//
// `_entry_raw` addresses mcode
// in `_module`'s mmap'd page. Because `_module` ships with the
// handle (the handle owns it by-value as a `SendJitModule`), the
// pointer remains dereferenceable on whichever OS thread the
// handle lands on after a Vm move. The pointer is read-only on
// the dispatch hot path (transmuted to an `extern "C"` fn and
// called); no aliasing concerns. Per-dispatch `JIT_VM` / `JIT_CL`
// TLS slots are scoped via `scoped_jit_vm_rebind` RAII so
// any thread that calls into the dispatcher re-arms its own slot.
// Mirror impl: `unsafe impl Send for JitHandle` at
// `jit_backend/mod.rs` just after the `JitHandle` struct.
unsafe impl Send for TraceHandle {}

/// Placeholder `TraceFn` — installed in
/// [`CompiledTraceParts::entry`] by the backend-agnostic [`lower_trace_into`]
/// body and patched to the real finalized entry pointer by the JIT
/// wrapper [`try_compile_trace_with_options`]. The AOT pipeline
/// (`luna-aot`) never calls into a TraceFn directly (resolution happens
/// at the deploy-side runtime, not at codegen time), so the placeholder
/// reaching the AOT output is harmless; the deploy-side runtime
/// resolves the trace symbol through the static dispatch table built in
/// `luna-aot`'s embed pipeline.
pub(super) unsafe extern "C" fn placeholder_trace_fn(_reg_state: *mut i64) -> i64 {
    panic!(
        "placeholder_trace_fn called: CompiledTraceParts.entry must be patched by the JIT finalize wrapper before dispatch"
    );
}

/// Whether anything can enter `ct` once it is cached: the dispatcher
/// admits it at its head (dispatchable, or linked for down-recursion), or
/// it is a side trace whose parent's exit calls it, which the Vm allows
/// for a trace that is only too short to dispatch on its own.
pub(crate) fn trace_is_enterable(record: &TraceRecord, ct: &CompiledTraceParts) -> bool {
    ct.dispatchable
        || ct.downrec_link.is_some()
        || (record.side_trace_parent.is_some() && ct.dispatch_off_reason == Some("length-gate"))
}
