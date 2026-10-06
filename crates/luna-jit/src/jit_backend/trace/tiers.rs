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
    compile_trace_captured(storage, record, opts, always_codegen, float_only, false).map(|r| r.0)
}

/// [`compile_trace_jit`], also handing back the code when `capture` (for a
/// Vm that shares its traces; see `super::share`).
pub(super) fn compile_trace_captured(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    always_codegen: bool,
    float_only: bool,
    capture: bool,
) -> Option<(CompiledTrace, Option<image::Captured>)> {
    if opts.tier != luna_core::jit::trace::TraceTier::Optimizing {
        match compile_trace_baseline(storage, record, opts, always_codegen, float_only, capture) {
            Ok(ct) => return ct,
            Err(why) => BASELINE_FALLBACK.with(|c| c.set((c.get().0 + 1, why))),
        }
    }
    #[cfg(feature = "llvm-jit")]
    if super::llvm_tier::is_llvm(storage) {
        return super::llvm_tier::compile_trace_llvm(
            storage,
            record,
            opts,
            always_codegen,
            float_only,
        )
        .map(|ct| (ct, None));
    }
    compile_trace_cranelift(storage, record, opts, always_codegen, float_only, capture)
}

type Compiled = Option<(CompiledTrace, Option<image::Captured>)>;

/// The baseline tier: `Ok(None)` when the record cannot be lowered at all,
/// `Err` when the baseline code generator cannot take it.
fn compile_trace_baseline(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    always_codegen: bool,
    float_only: bool,
    capture: bool,
) -> Result<Compiled, &'static str> {
    let Some((lir, mut compiled)) = lower_trace_lir(record, opts, float_only) else {
        return Ok(None);
    };
    if !always_codegen && !trace_is_enterable(record, &compiled) {
        lir.give();
        let cap = capture.then(|| image::Captured {
            code: None,
            relocs: Vec::new(),
            lir: None,
        });
        return Ok(Some((compiled, cap)));
    }
    let Ok(cs) = crate::jit_backend::storage::from_storage(storage) else {
        lir.give();
        return Ok(None);
    };
    let placed = lir::assemble(&lir, &mut cs.baseline_code, capture);
    let shared_lir = compiled
        .tier_up
        .as_ref()
        .map(|_| std::sync::Arc::new(lir.detach()));
    if let (Some(t), Some(l)) = (&compiled.tier_up, &shared_lir) {
        *t.source.borrow_mut() = Some(Box::new(share::TierSource {
            lir: l.clone(),
            relocs: lir.relocs.clone(),
            image: None,
        }));
    }
    let relocs = lir.relocs.clone();
    lir.give();
    let (entry, code) = placed?;
    BASELINE_CODEGEN.with(|c| c.set(c.get() + 1));
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    // SAFETY: the code implements the `TraceFn` ABI (`extern "C"`, one
    // pointer argument, an i64 result); it stays mapped until the owning
    // Vm releases its code
    compiled.entry = unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) };
    let cap = code.map(|c| image::Captured {
        code: Some((image::Tier::Baseline, c)),
        relocs,
        lir: shared_lir,
    });
    Ok(Some((compiled, cap)))
}

pub(super) fn compile_trace_cranelift(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    always_codegen: bool,
    float_only: bool,
    capture: bool,
) -> Compiled {
    let mut module =
        crate::jit_backend::send_jit_module::UnpublishedModule::new(build_trace_jit_module()?);
    let (fn_id, mut compiled) =
        lower_trace_into_inner(&mut *module, record, opts, None, always_codegen, float_only)?;
    if !always_codegen && !trace_is_enterable(record, &compiled) {
        let cap = capture.then(|| image::Captured {
            code: None,
            relocs: Vec::new(),
            lir: None,
        });
        return Some((compiled, cap));
    }
    let relocs = reloc::values();
    module.finalize_definitions().ok()?;
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    let ptr = module.get_finalized_function(fn_id);
    super::code_dump::dump("cranelift", record.head_pc, ptr);
    // SAFETY: the cranelift fn signature declared by `lower_trace_into`
    // (`(I64) -> I64`) matches `TraceFn`. The mmap backing the fn body
    // is owned by `module`, which we park on the per-`Vm` storage's
    // `trace_handles` Vec immediately below.
    let entry_fn: TraceFn = unsafe { std::mem::transmute::<*const u8, TraceFn>(ptr) };
    compiled.entry = entry_fn;
    let sites = reloc::take_sites();
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
    let cap = match sites {
        Some((len, sites)) if capture => Some(image::Captured {
            // SAFETY: `ptr..ptr + len` is the function just finalized, which
            // the storage keeps mapped
            code: Some((image::Tier::Optimizing, unsafe {
                reloc::copy_code(ptr, len, sites)
            })),
            relocs,
            lir: None,
        }),
        _ => None,
    };
    Some((compiled, cap))
}

/// Compiles a baseline trace again with Cranelift, from the instructions the
/// baseline tier ran (see `TraceCompiler::tier_up`).
pub(crate) fn tier_up_trace(
    storage: &mut dyn luna_core::jit::JitStorage,
    ct: &CompiledTrace,
) -> Option<TraceFn> {
    share::tier_up(storage, ct)
}
