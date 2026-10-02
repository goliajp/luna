//! Which code generator compiles a trace: the baseline tier, or Cranelift.

use super::*;

thread_local! {
    static BASELINE_CODEGEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static BASELINE_FALLBACK: std::cell::Cell<(u64, &'static str)> =
        const { std::cell::Cell::new((0, "")) };
}

/// Traces this thread compiled with the baseline code generator.
#[doc(hidden)]
pub fn baseline_codegen_count() -> u64 {
    BASELINE_CODEGEN.with(|c| c.get())
}

/// Traces the baseline code generator handed to Cranelift on this thread,
/// and the reason for the last one.
#[doc(hidden)]
pub fn baseline_fallback() -> (u64, &'static str) {
    BASELINE_FALLBACK.with(|c| c.get())
}

/// `float_only`: the record's dialect is 5.1 / 5.2, where the math
/// library converts its number arguments to floats and returns floats
/// (`math.min(1, 2)` is the float `1`).
pub(super) fn compile_trace_jit(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    always_codegen: bool,
    float_only: bool,
) -> Option<CompiledTrace> {
    if opts.tier != luna_core::jit::trace::TraceTier::Optimizing {
        match compile_trace_baseline(storage, record, opts, always_codegen, float_only) {
            Ok(ct) => return ct,
            Err(why) => BASELINE_FALLBACK.with(|c| c.set((c.get().0 + 1, why))),
        }
    }
    compile_trace_cranelift(storage, record, opts, always_codegen, float_only)
}

/// The baseline tier: `Ok(None)` when the record cannot be lowered at all,
/// `Err` when the baseline code generator cannot take it.
fn compile_trace_baseline(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    always_codegen: bool,
    float_only: bool,
) -> Result<Option<CompiledTrace>, &'static str> {
    let Some((lir, mut compiled)) = lower_trace_lir(record, opts, float_only) else {
        return Ok(None);
    };
    if !always_codegen && !trace_is_enterable(record, &compiled) {
        lir.give();
        return Ok(Some(compiled));
    }
    let code = lir::assemble(&lir);
    lir.give();
    let code = code?;
    BASELINE_CODEGEN.with(|c| c.set(c.get() + 1));
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    // SAFETY: the code implements the `TraceFn` ABI (`extern "C"`, one
    // pointer argument, an i64 result); it stays mapped while `storage`
    // holds `code`, which is until the owning Vm releases its code
    compiled.entry = unsafe { std::mem::transmute::<*const u8, TraceFn>(code.entry) };
    let Ok(cs) = crate::jit_backend::storage::from_storage(storage) else {
        // SAFETY: nothing has seen the entry
        unsafe { code.free() };
        return Ok(None);
    };
    cs.baseline_code.push(code);
    Ok(Some(compiled))
}

pub(super) fn compile_trace_cranelift(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    always_codegen: bool,
    float_only: bool,
) -> Option<CompiledTrace> {
    let mut module =
        crate::jit_backend::send_jit_module::UnpublishedModule::new(build_trace_jit_module()?);
    let (fn_id, mut compiled) =
        lower_trace_into_inner(&mut *module, record, opts, None, always_codegen, float_only)?;
    if !always_codegen && !trace_is_enterable(record, &compiled) {
        return Some(compiled);
    }
    module.finalize_definitions().ok()?;
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    let ptr = module.get_finalized_function(fn_id);
    // SAFETY: the cranelift fn signature declared by `lower_trace_into`
    // (`(I64) -> I64`) matches `TraceFn`. The mmap backing the fn body
    // is owned by `module`, which we park on the per-`Vm` storage's
    // `trace_handles` Vec immediately below.
    let entry_fn: TraceFn = unsafe { std::mem::transmute::<*const u8, TraceFn>(ptr) };
    compiled.entry = entry_fn;
    // `from_storage` is `Result`-shaped. On
    // `StorageMismatch` (Vm.jit.storage isn't a CraneliftJitStorage)
    // skip parking the handle and return `None` — the freshly built
    // `module` drops here and releases its mmap pages; the trace
    // recorder sees `None` and gives up on this trace, falling back
    // to interp dispatch. No SIGABRT across the C-ABI boundary.
    let cs = crate::jit_backend::storage::from_storage(storage).ok()?;
    cs.trace_handles.push(TraceHandle {
        // Wrap in `SendJitModule` sleeve.
        _module: module.publish(),
        _entry_raw: ptr,
    });
    Some(compiled)
}
