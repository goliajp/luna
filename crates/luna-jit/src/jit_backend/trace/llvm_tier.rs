//! The LLVM backend's optimizing tier: the shared trace lowering, recorded
//! as for the baseline tier, compiled by LLVM where the Cranelift backend
//! uses Cranelift.

use super::*;

thread_local! {
    static LLVM_CODEGEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Traces (and tier-ups of baseline traces) this thread compiled with LLVM.
#[doc(hidden)]
pub fn llvm_codegen_count() -> u64 {
    LLVM_CODEGEN.with(|c| c.get())
}

/// Whether `storage` is the LLVM backend's.
pub(super) fn is_llvm(storage: &mut dyn luna_core::jit::JitStorage) -> bool {
    crate::jit_backend::storage::from_storage(storage).is_ok_and(|cs| cs.llvm.is_some())
}

/// Compiles `lir`, keeps the code in `storage` and returns its entry.
/// `None` (with the reason as the checkpoint) when LLVM did not take it.
fn codegen(
    storage: &mut dyn luna_core::jit::JitStorage,
    lir: &lir::Lir,
    relocs: &[(RelocKind, i64)],
) -> Option<TraceFn> {
    let (entry, pair) = match lir::compile_llvm(lir, relocs) {
        Ok(c) => c,
        Err(why) => {
            checkpoint(why);
            return None;
        }
    };
    let cs = crate::jit_backend::storage::from_storage(storage).ok()?;
    cs.llvm.as_mut()?.park_engine(pair);
    LLVM_CODEGEN.with(|c| c.set(c.get() + 1));
    TRACE_CODEGEN.with(|c| c.set(c.get() + 1));
    // SAFETY: the function implements the `TraceFn` ABI (`extern "C"`, one
    // pointer-sized integer argument, an i64 result: see `lir::llvm`); the
    // storage keeps its code mapped while the Vm holds the trace
    Some(unsafe { std::mem::transmute::<*const u8, TraceFn>(entry) })
}

/// [`super::tiers::compile_trace_cranelift`] for the LLVM backend.
pub(super) fn compile_trace_llvm(
    storage: &mut dyn luna_core::jit::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    always_codegen: bool,
    float_only: bool,
) -> Option<CompiledTrace> {
    let opts = CompileOptions {
        tier: TraceTier::Optimizing,
        ..opts
    };
    let (lir, mut compiled) = lower_trace_lir(record, opts, float_only)?;
    if !always_codegen && !trace_is_enterable(record, &compiled) {
        lir.give();
        return Some(compiled);
    }
    let entry = codegen(storage, &lir, &lir.relocs);
    lir.give();
    compiled.entry = entry?;
    Some(compiled)
}

/// [`super::share::tier_up`] for the LLVM backend: the baseline trace
/// `ct` compiled again by LLVM.
pub(super) fn tier_up_llvm(
    storage: &mut dyn luna_core::jit::JitStorage,
    ct: &CompiledTrace,
) -> Option<TraceFn> {
    let source = ct.tier_up.as_ref()?.source.borrow_mut().take()?;
    let src = source.downcast::<share::TierSource>().ok()?;
    let entry = codegen(storage, &src.lir, &src.relocs)?;
    super::code_dump::dump("tier-up-llvm", ct.head_pc, entry as *const u8);
    Some(entry)
}
