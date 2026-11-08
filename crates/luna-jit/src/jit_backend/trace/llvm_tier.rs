//! The LLVM backend's optimizing tier: the shared trace lowering, recorded
//! as for the baseline tier, compiled by LLVM where the Cranelift backend
//! uses Cranelift.
//!
//! LLVM takes milliseconds per trace where Cranelift takes a fraction of
//! one, so a hot baseline trace is by default compiled again on a thread
//! of its own while Cranelift's code for it runs; the Vm picks the new
//! code up the next time the trace is entered (see
//! `TraceCompiler::tier_up`).

use super::tier_source::{TierSource, calls_of, expanded, kept, of};
use super::*;
use luna_core::__private::TierKept;
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
pub(super) fn is_llvm(storage: &mut dyn luna_core::__private::JitStorage) -> bool {
    crate::jit_backend::storage::from_storage(storage).is_ok_and(|cs| cs.llvm.is_some())
}

type Compiled = Result<(usize, luna_jit_llvm::EnginePair), &'static str>;

/// Keeps LLVM's code in `storage` and returns its entry. `None` (with the
/// reason as the checkpoint) when LLVM did not take the trace.
fn install(storage: &mut dyn luna_core::__private::JitStorage, c: Compiled) -> Option<TraceFn> {
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

fn compile(
    lir: &lir::Lir,
    relocs: &[(RelocKind, i64)],
    start: lir::Start,
    calls: &lir::Calls,
) -> Compiled {
    lir::compile_llvm(lir, relocs, start, calls).map(|(e, p)| (e as usize, p))
}

/// The optimizing tier's code of a baseline trace: for one whose back edge
/// calls the optimizing tier's loop (see `lower::handoff`), the loop and
/// the first pass, else the whole trace.
type Tiered = (Option<Compiled>, Compiled);

fn compile_tier((lir, relocs): (&lir::Lir, &[(RelocKind, i64)]), calls: &lir::Calls) -> Tiered {
    match lir.loop_entry.is_some() {
        true => (
            Some(compile(lir, relocs, lir::Start::Loop, calls)),
            compile(lir, relocs, lir::Start::FirstPass, calls),
        ),
        false => (None, compile(lir, relocs, lir::Start::Whole, calls)),
    }
}

/// Installs `t` for `ct`: the loop into its loop cell, then the entry.
fn install_tier(
    storage: &mut dyn luna_core::__private::JitStorage,
    ct: &CompiledTrace,
    t: Tiered,
) -> Option<TraceFn> {
    if let Some(l) = t.0 {
        let p = install(storage, l)?;
        // SAFETY: LLVM's loop of this trace, kept mapped by the storage
        unsafe { super::tier_source::set_loop(ct, p as *const u8) };
    }
    install(storage, t.1)
}

/// [`super::tiers::compile_trace_cranelift`] for the LLVM backend.
pub(super) fn compile_trace_llvm(
    storage: &mut dyn luna_core::__private::JitStorage,
    record: &TraceRecord,
    opts: CompileOptions,
    always_codegen: bool,
    float_only: bool,
) -> Option<CompiledTraceParts> {
    let opts = CompileOptions {
        tier: TraceTier::Optimizing,
        ..opts
    };
    let (lir, mut compiled) = lower_trace_lir(record, opts, float_only, true)?;
    if !always_codegen && !trace_is_enterable(record, &compiled) {
        lir.give();
        return Some(compiled);
    }
    // every calling exit calls: the cells are made now
    let ex = lir.relinks.is_some().then(|| {
        let cells = luna_core::__private::side_cells(compiled.side_cell_exits.len());
        let l = lir::expand(&lir, &lir.relocs, cells.as_ptr() as i64);
        compiled.side_cells = Some(cells);
        l
    });
    let l = ex.as_ref().unwrap_or(&lir);
    let c = compile(l, &l.relocs, lir::Start::Whole, &lir::Calls::All);
    lir.give();
    compiled.entry = install(storage, c)?;
    Some(compiled)
}

/// A trace running Cranelift's code until LLVM's is ready; the trace's
/// tier source meanwhile.
struct Pending {
    /// What LLVM compiles, until it is handed to the compile thread.
    source: Option<Box<TierSource>>,
    /// What was handed to the compile thread and the calling exits it
    /// compiles for: kept once LLVM's code is installed.
    handed: Option<(Box<TierSource>, lir::Calls)>,
    /// Set by the compile thread once `delay` has passed since the tier-up.
    due: Arc<AtomicBool>,
    /// Set by the compile thread once `done` holds the result.
    ready: Arc<AtomicBool>,
    done: Arc<Mutex<Option<Tiered>>>,
}

type TierUp = (Option<TraceFn>, Option<TierKept>);

/// [`super::tier_source::tier_up`] for the LLVM backend: the baseline trace `ct`
/// compiled again by LLVM.
///
/// With `delay`, the first call compiles the trace with Cranelift at once,
/// as the Cranelift backend would, and the Vm asks again at each entry of
/// the trace. The first entry after `delay` (the compile thread marks the
/// moment) hands the trace to the compile thread, and the first entry after
/// LLVM's code is ready switches to it. A trace not entered after `delay`
/// never costs an LLVM compile; a loop that runs on in one entry keeps
/// Cranelift's code until it is entered again. Each entry only reads
/// flags. Without `delay`, LLVM compiles the trace before it runs on.
pub(crate) fn tier_up_llvm(
    storage: &mut dyn luna_core::__private::JitStorage,
    ct: &CompiledTrace,
    delay: Option<std::time::Duration>,
) -> TierUp {
    let Some(source) = luna_core::__private::take_tier_source(ct) else {
        return (None, None);
    };
    let source = match source.downcast::<Pending>() {
        Ok(p) => return pending(storage, ct, p),
        Err(s) => match s.downcast::<TierSource>() {
            Ok(s) => s.into_optimizing(),
            Err(_) => return (None, None),
        },
    };
    let calls = calls_of(ct);
    let Some(delay) = delay else {
        let ex = expanded(&source, ct, &calls);
        let entry = install_tier(storage, ct, compile_tier(of(&source, &ex), &calls));
        if let Some(e) = entry {
            super::code_dump::dump("tier-up-llvm", ct.head_pc(), e as *const u8);
        }
        return (entry, entry.and_then(|_| kept(source, &calls, false)));
    };
    let entry = super::tier_source::clif_tier_up(storage, &source, ct, &calls);
    let mut p = Pending {
        source: None,
        handed: None,
        due: Arc::default(),
        ready: Arc::default(),
        done: Arc::default(),
    };
    let now = std::time::Instant::now();
    if delay.is_zero() {
        hand_over(storage, &mut p, source, ct, now);
    } else {
        p.source = Some(source);
        let due = p.due.clone();
        let ticket = crate::jit_backend::llvm_thread::submit(
            Box::new(move || due.store(true, Ordering::Relaxed)),
            now + delay,
        );
        push_ticket(storage, ticket);
    }
    // SAFETY: the pending compile of the source taken from `ct` above
    unsafe { luna_core::__private::put_tier_source(ct, Box::new(p)) };
    (entry, None)
}

/// A later entry of a trace whose LLVM compile is pending: LLVM's code
/// once it is ready, with what the trace keeps to compile again, else
/// `None` (handing the source to the compile thread once `delay` has
/// passed).
fn pending(
    storage: &mut dyn luna_core::__private::JitStorage,
    ct: &CompiledTrace,
    mut p: Box<Pending>,
) -> TierUp {
    if p.ready.load(Ordering::Acquire) {
        let Some(c) = p.done.lock().expect(POISON).take() else {
            return (None, None);
        };
        // written on the compile thread
        luna_core::jit::code_fence();
        let entry = install_tier(storage, ct, c);
        let Some(e) = entry else {
            return (None, None);
        };
        super::code_dump::dump("tier-up-llvm", ct.head_pc(), e as *const u8);
        let kept = p.handed.take().and_then(|(src, calls)| {
            let stale = calls != calls_of(ct);
            kept(src, &calls, stale)
        });
        return (entry, kept);
    }
    if p.due.load(Ordering::Relaxed)
        && let Some(src) = p.source.take()
    {
        let now = std::time::Instant::now();
        hand_over(storage, &mut p, src, ct, now);
    }
    // SAFETY: the pending compile of the source just taken from `ct`
    unsafe { luna_core::__private::put_tier_source(ct, p) };
    (None, None)
}

/// Has the compile thread compile `src` for `p`, its calling exits calling
/// as those of `ct` do now.
fn hand_over(
    storage: &mut dyn luna_core::__private::JitStorage,
    p: &mut Pending,
    src: Box<TierSource>,
    ct: &CompiledTrace,
    now: std::time::Instant,
) {
    let (ready, done) = (p.ready.clone(), p.done.clone());
    let calls = calls_of(ct);
    let job = match expanded(&src, ct, &calls) {
        Some(l) => (Arc::new(l), None),
        None => (src.lir.clone(), Some(src.relocs.clone())),
    };
    let c = calls.clone();
    let ticket = crate::jit_backend::llvm_thread::submit(
        Box::new(move || {
            let relocs = job.1.as_deref().unwrap_or(&job.0.relocs);
            *done.lock().expect(POISON) = Some(compile_tier((&job.0, relocs), &c));
            ready.store(true, Ordering::Release);
        }),
        now,
    );
    push_ticket(storage, ticket);
    p.handed = Some((src, calls));
}
fn push_ticket(
    storage: &mut dyn luna_core::__private::JitStorage,
    ticket: crate::jit_backend::llvm_thread::Ticket,
) {
    if let Ok(cs) = crate::jit_backend::storage::from_storage(storage) {
        cs.llvm_tickets.push(ticket);
    }
}

const POISON: &str = "a job never panics holding its result";
