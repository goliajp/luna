//! The LLVM backend's optimizing tier: the shared trace lowering, recorded
//! as for the baseline tier, compiled by LLVM where the Cranelift backend
//! uses Cranelift.
//!
//! LLVM takes milliseconds per trace where Cranelift takes a fraction of
//! one, so a hot baseline trace is by default compiled again on a thread
//! of its own while the baseline code keeps running; the Vm picks the new
//! code up the next time the trace reaches its tier-up count (see
//! `TraceCompiler::tier_up`).

use super::*;
use std::sync::{Arc, Mutex, mpsc};

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

/// A trace being compiled on the compile thread; what [`TierUp::source`]
/// holds meanwhile.
struct Pending(Arc<Mutex<Option<Compiled>>>);

/// A trace running Cranelift's code, not yet handed to LLVM: since when.
struct Waiting {
    source: Box<share::TierSource>,
    since: std::time::Instant,
}

struct Job {
    source: Box<share::TierSource>,
    done: Arc<Mutex<Option<Compiled>>>,
}

/// Jobs submitted and not finished yet; the compile thread signals the
/// condition variable when the count drops.
static IN_FLIGHT: (Mutex<usize>, std::sync::Condvar) = (Mutex::new(0), std::sync::Condvar::new());

/// The compile thread, started by the first background tier-up. A job whose
/// trace is gone (nothing holds its result any more) is dropped unrun.
fn submit(job: Job) -> Option<()> {
    static QUEUE: std::sync::OnceLock<Mutex<mpsc::Sender<Job>>> = std::sync::OnceLock::new();
    let q = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("luna-llvm-tier".into())
            .spawn(move || {
                for job in rx {
                    if Arc::strong_count(&job.done) > 1 {
                        let c = compile(&job.source.lir, &job.source.relocs);
                        *job.done.lock().expect(POISON) = Some(c);
                    }
                    drop(job);
                    *IN_FLIGHT.0.lock().expect(POISON) -= 1;
                    IN_FLIGHT.1.notify_all();
                }
            })
            .expect("starting the LLVM compile thread");
        Mutex::new(tx)
    });
    *IN_FLIGHT.0.lock().expect(POISON) += 1;
    let sent = q.lock().expect(POISON).send(job);
    if sent.is_err() {
        *IN_FLIGHT.0.lock().expect(POISON) -= 1;
    }
    sent.ok()
}

const POISON: &str = "the compile thread never panics holding a lock";

/// Waits until the compile thread has nothing left to do. A Vm of the LLVM
/// backend calls this as it goes away: LLVM must not be running on the
/// compile thread when the process exits and LLVM's global state is torn
/// down, and its jobs' traces are gone by then, so the queue drains fast.
pub(crate) fn quiesce() {
    let mut n = IN_FLIGHT.0.lock().expect(POISON);
    while *n > 0 {
        n = IN_FLIGHT.1.wait(n).expect(POISON);
    }
}

/// [`super::share::tier_up`] for the LLVM backend: the baseline trace `ct`
/// compiled again by LLVM.
///
/// With `delay`, the first call compiles the trace with Cranelift at once,
/// as the Cranelift backend would; Cranelift's code counts iterations like
/// the baseline tier's, so the Vm keeps asking every tier-up count. Once
/// the trace has kept running that code for `delay`, LLVM compiles it on
/// the compile thread, and the Vm installs LLVM's code when it is ready. A
/// trace that stops being hot within `delay` never costs an LLVM compile.
/// Without, LLVM compiles the trace before it runs on.
pub(crate) fn tier_up_llvm(
    storage: &mut dyn luna_core::jit::JitStorage,
    ct: &CompiledTrace,
    delay: Option<std::time::Duration>,
) -> Option<TraceFn> {
    let t = ct.tier_up.as_ref()?;
    let source = t.source.borrow_mut().take()?;
    let source = match source.downcast::<Pending>() {
        Ok(p) => {
            let done = p.0.lock().expect(POISON).take();
            let Some(c) = done else {
                *t.source.borrow_mut() = Some(p);
                return None;
            };
            let entry = install(storage, c)?;
            super::code_dump::dump("tier-up-llvm", ct.head_pc, entry as *const u8);
            return Some(entry);
        }
        Err(s) => s,
    };
    let source = match source.downcast::<Waiting>() {
        Ok(w) if delay.is_some_and(|d| w.since.elapsed() < d) => {
            *t.source.borrow_mut() = Some(w);
            return None;
        }
        Ok(w) => {
            let done = Arc::new(Mutex::new(None));
            submit(Job {
                source: w.source,
                done: done.clone(),
            })?;
            *t.source.borrow_mut() = Some(Box::new(Pending(done)));
            return None;
        }
        Err(s) => s.downcast::<share::TierSource>().ok()?,
    };
    let Some(_) = delay else {
        let entry = install(storage, compile(&source.lir, &source.relocs))?;
        super::code_dump::dump("tier-up-llvm", ct.head_pc, entry as *const u8);
        return Some(entry);
    };
    let entry = share::clif_tier_up(storage, &source, ct.head_pc, true);
    let lir = source.lir.clone();
    let relocs = source.relocs.clone();
    *t.source.borrow_mut() = Some(Box::new(Waiting {
        source: Box::new(share::TierSource {
            lir,
            relocs,
            image: None,
        }),
        since: std::time::Instant::now(),
    }));
    entry
}
