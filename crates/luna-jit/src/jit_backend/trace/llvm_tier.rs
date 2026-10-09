//! The LLVM backend's optimizing tier: the shared trace lowering, recorded
//! as for the baseline tier, compiled by LLVM where the Cranelift backend
//! uses Cranelift.
//!
//! LLVM takes milliseconds per trace where Cranelift takes a fraction of
//! one, so a hot baseline trace is by default compiled again on a thread
//! of its own while Cranelift's code for it runs; the Vm picks the new
//! code up the next time the trace is entered (see
//! `TraceCompiler::tier_up`).

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

thread_local! {
    static LLVM_CODEGEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Traces (and tier-ups of baseline traces) whose LLVM code this thread
/// installed.
#[doc(hidden)]
pub fn llvm_codegen_count() -> u64 {
    LLVM_CODEGEN.with(|c| c.get())
}

/// Whether `storage` is the LLVM backend's.
pub(super) fn is_llvm(storage: &mut dyn luna_core::jit::JitStorage) -> bool {
    crate::jit_backend::storage::from_storage(storage).is_ok_and(|cs| cs.llvm.is_some())
}

type Compiled = Result<(usize, luna_jit_llvm::EnginePair), &'static str>;

/// Keeps LLVM's code in `storage` and returns its entry. `None` (with the
/// reason as the checkpoint) when LLVM did not take the trace.
fn install(storage: &mut dyn luna_core::jit::JitStorage, c: Compiled) -> Option<TraceFn> {
    let (entry, pair) = match c {
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
    Some(unsafe { std::mem::transmute::<usize, TraceFn>(entry) })
}

fn compile(lir: &lir::Lir, relocs: &[(RelocKind, i64)]) -> Compiled {
    lir::compile_llvm(lir, relocs).map(|(e, p)| (e as usize, p))
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
    let c = compile(&lir, &lir.relocs);
    lir.give();
    compiled.entry = install(storage, c)?;
    Some(compiled)
}

/// A trace running Cranelift's code while LLVM compiles it on the compile
/// thread; what [`TierUp::source`] holds meanwhile.
struct Pending {
    /// Set by the compile thread once `done` holds the result.
    ready: Arc<AtomicBool>,
    done: Arc<Mutex<Option<Compiled>>>,
    /// Set at each entry of the trace: the job compiles only a trace
    /// entered again while it waited.
    entered: Arc<AtomicBool>,
}

/// [`super::share::tier_up`] for the LLVM backend: the baseline trace `ct`
/// compiled again by LLVM.
///
/// With `delay`, the first call compiles the trace with Cranelift at once,
/// as the Cranelift backend would, and hands it to the compile thread to
/// compile with LLVM after `delay`; the Vm asks again at each entry of the
/// trace and switches to LLVM's code at the first entry after it is ready.
/// A trace not entered again within `delay` never costs an LLVM compile; a
/// loop that runs on in one entry keeps Cranelift's code until it is
/// entered again. Without, LLVM compiles the trace before it runs on.
pub(crate) fn tier_up_llvm(
    storage: &mut dyn luna_core::jit::JitStorage,
    ct: &CompiledTrace,
    delay: Option<std::time::Duration>,
) -> Option<TraceFn> {
    let t = ct.tier_up.as_ref()?;
    let source = t.source.borrow_mut().take()?;
    let source = match source.downcast::<Pending>() {
        Ok(p) => {
            p.entered.store(true, Ordering::Relaxed);
            if !p.ready.load(Ordering::Acquire) {
                *t.source.borrow_mut() = Some(p);
                return None;
            }
            let c = p.done.lock().expect(POISON).take()?;
            // written on the compile thread
            luna_core::jit::code_fence();
            let entry = install(storage, c)?;
            super::code_dump::dump("tier-up-llvm", ct.head_pc, entry as *const u8);
            return Some(entry);
        }
        Err(s) => s.downcast::<share::TierSource>().ok()?,
    };
    let Some(delay) = delay else {
        let entry = install(storage, compile(&source.lir, &source.relocs))?;
        super::code_dump::dump("tier-up-llvm", ct.head_pc, entry as *const u8);
        return Some(entry);
    };
    let entry = share::clif_tier_up(storage, &source, ct.head_pc);
    let p = Pending {
        ready: Arc::default(),
        done: Arc::default(),
        entered: Arc::default(),
    };
    let (ready, done, entered) = (p.ready.clone(), p.done.clone(), p.entered.clone());
    let (lir, relocs) = (source.lir.clone(), source.relocs.clone());
    let ticket = crate::jit_backend::llvm_thread::submit(
        Box::new(move || {
            let c = if entered.load(Ordering::Relaxed) {
                compile(&lir, &relocs)
            } else {
                Err("llvm:not-entered-again")
            };
            *done.lock().expect(POISON) = Some(c);
            ready.store(true, Ordering::Release);
        }),
        std::time::Instant::now() + delay,
    );
    crate::jit_backend::storage::from_storage(storage)
        .ok()?
        .llvm_tickets
        .push(ticket);
    *t.source.borrow_mut() = Some(Box::new(p));
    entry
}

const POISON: &str = "a job never panics holding its result";
