//! SS-A — Send-wrapper overhead baseline.
//!
//! Measures the irreducible cost of wrapping a `Vm` behind a `Send`-shaped
//! indirection layer (`Arc<UnsafeCell<Vm>>` plus `unsafe impl Send`) **before
//! any real Arc-of-fields / RwLock semantics land**. This is the framework
//! tax that Phase SS-B `SendVm` will pay on top of (or instead of) the
//! `Gc<T>` per-deref cost decomposed in `.dev/rfcs/v1.2-audit-send-cost.md`.
//!
//! ## What this bench is not
//!
//! - **Not** a real `SendVm` implementation. The wrapper here is a
//!   shape-only no-op: same `Vm`, same `NonNull<T>` GC handles, same
//!   single-mutator dispatcher. Only difference vs bare `Vm` is one
//!   `Arc` load + one `UnsafeCell::get()` per outer call.
//! - **Not** thread-safe in any meaningful sense — the `unsafe impl Send`
//!   exists only so the wrapper has the right *type-level* shape; the
//!   bench drives it from one thread. (Genuine cross-thread tests land
//!   in SS-D.)
//!
//! ## Reading the output
//!
//! Two pairs are measured:
//!
//! - `bare_vm_eval` vs `wrapped_vm_eval` — minimal-eval cost ratio
//! - `bare_vm_token_bucket` vs `wrapped_vm_token_bucket` — real workload ratio
//!
//! The audit (`.dev/rfcs/v1.3-audit-send-vm-design.md` §3.2) projects:
//!
//! | arch    | per-deref `Arc<UnsafeCell<T>>` cost | full SendVm regression projection (B2) |
//! |---------|-------------------------------------|----------------------------------------|
//! | ARM M-series | ~3 ns | ~3 % |
//! | x86_64 Linux | ~6 ns | ~6 % |
//!
//! This bench measures only the **outer Vm-handle** indirection, not the
//! per-`Gc<T>` deref cost (that lands in SS-A.1 luna-core bench separately).
//! If `wrapped_vm_token_bucket` overhead exceeds 5 % on macOS at this
//! handle-only layer, flag it: it means the SS-B SendVm projection in the
//! audit is optimistic and SS-B should re-scope. (Counterpart: < 1 %
//! confirms the framework itself is cheap and the SS-B cost budget can
//! focus entirely on per-`Gc<T>` deref.)
//!
//! ## Linux taskset note
//!
//! macOS runs use criterion's default (no public CPU pin API on
//! Apple Silicon). For Linux, the `perf-gate` job in
//! `.github/workflows/ci.yml` already wraps `cargo bench` with
//! `taskset -c 1` to drive variance under ~1-2 %. To capture the
//! authoritative x86_64 number, dispatch that workflow against this
//! bench name (`bench_send_overhead`).
//!
//! Run:
//!   `cargo bench --bench bench_send_overhead`
//!   `cargo bench --bench bench_send_overhead -- wrapped_vm_token_bucket`

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use luna_jit::Vm;
use luna_jit::version::LuaVersion;

// ── NoOpSendWrapper ────────────────────────────────────────────────────
//
// Shape mirror for SS-B `SendVm`: holds the Vm behind an `Arc<UnsafeCell>`
// indirection so the wrapper type can be `Send`-shaped even though the
// inner `Vm` is `!Send`. SAFETY of the `unsafe impl Send` claim below is
// **not** a real safety story — it is a measurement scaffold. The bench
// only drives the wrapper from a single thread; never clone the `Arc` and
// move both ends across threads. The real SS-B `SendVm` will earn `Send`
// via per-field Arc-ification (audit §3.2), not via this fiction.
//
struct NoOpSendWrapper {
    inner: Arc<UnsafeCell<Vm>>,
}

// SAFETY: see module-level note. This impl exists *only* so the wrapper has
// the same type-level shape as the future SS-B `SendVm` for codegen-cost
// measurement; the bench drives it from one thread and never shares it.
unsafe impl Send for NoOpSendWrapper {}

impl NoOpSendWrapper {
    fn new(vm: Vm) -> Self {
        // clippy::arc_with_non_send_sync fires because `UnsafeCell<Vm>` is
        // !Send + !Sync. That's exactly the type-shape we want to measure
        // — see module preamble. The `unsafe impl Send for NoOpSendWrapper`
        // below carries the (single-thread-only) safety story.
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(UnsafeCell::new(vm));
        Self { inner }
    }

    /// Mirror of `Vm::eval` reached through the Arc+UnsafeCell indirection.
    /// One extra `Arc::deref` (ptr load) + one `UnsafeCell::get` (ptr cast)
    /// + one `&mut` materialization compared to bare `Vm::eval`. No locks,
    /// no atomics on the hot path beyond the Arc's strong-count touch at
    /// clone/drop (which doesn't happen inside `eval`).
    fn eval(
        &mut self,
        src: &str,
    ) -> Result<Vec<luna_jit::runtime::Value>, luna_jit::vm::error::LuaError> {
        // SAFETY: single-thread bench harness; no aliasing `&mut Vm` exists
        // outside this call. (Real SendVm enforces this structurally via
        // `&mut self` on the outer handle.)
        let vm: &mut Vm = unsafe { &mut *self.inner.get() };
        vm.eval(src)
    }
}

// ── Workloads ──────────────────────────────────────────────────────────

/// Minimal-eval shape: compile + run "return 1+2". Stresses the per-call
/// overhead (loader + 1 frame + return) so the relative cost of the
/// wrapper indirection shows up.
const MINIMAL_EVAL: &str = "return 1+2";

/// The Redis-Lua token-bucket workload from `redis_lua_shape.rs`. Real
/// embedder shape; the framework-overhead measurement on this workload
/// is the load-bearing number for projecting Phase SS-B's full-stack cost.
const TOKEN_BUCKET_1K: &str = r#"
    local bucket = { tokens = 1000, last = 0, rate = 100 }
    local now = 1
    local refilled = 0
    for i = 1, 1000 do
        local elapsed = now - bucket.last
        local refill = elapsed * bucket.rate
        if refill > 0 then
            bucket.tokens = math.min(1000, bucket.tokens + refill)
            bucket.last = now
            refilled = refilled + 1
        end
        if bucket.tokens >= 1 then
            bucket.tokens = bucket.tokens - 1
        end
        now = now + 1
    end
    return bucket.tokens, refilled
"#;

// ── Setup ──────────────────────────────────────────────────────────────

fn fresh_vm() -> Vm {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.open_base();
    vm.open_math();
    vm.open_string();
    vm.open_table();
    vm
}

// ── Bench ──────────────────────────────────────────────────────────────

fn bench_send_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("send_overhead");

    // Mirror `redis_lua_shape.rs` discipline: long enough measurement_time
    // for outlier-rejection convergence, sample_size 100, 2.5% noise gate
    // (macOS; Linux taskset CI gate tightens this further).
    group.measurement_time(Duration::from_secs(8));
    group.warm_up_time(Duration::from_secs(2));
    group.sample_size(100);
    group.noise_threshold(0.025);

    // ── Pair 1: minimal eval ────────────────────────────────────────
    group.bench_function("bare_vm_eval", |bencher| {
        bencher.iter_batched(
            fresh_vm,
            |mut vm| {
                black_box(vm.eval(MINIMAL_EVAL).expect("bench script must run"));
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("wrapped_vm_eval", |bencher| {
        bencher.iter_batched(
            || NoOpSendWrapper::new(fresh_vm()),
            |mut w| {
                black_box(w.eval(MINIMAL_EVAL).expect("bench script must run"));
            },
            BatchSize::SmallInput,
        );
    });

    // ── Pair 2: token bucket (real workload shape) ──────────────────
    group.bench_function("bare_vm_token_bucket", |bencher| {
        bencher.iter_batched(
            fresh_vm,
            |mut vm| {
                black_box(vm.eval(TOKEN_BUCKET_1K).expect("bench script must run"));
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("wrapped_vm_token_bucket", |bencher| {
        bencher.iter_batched(
            || NoOpSendWrapper::new(fresh_vm()),
            |mut w| {
                black_box(w.eval(TOKEN_BUCKET_1K).expect("bench script must run"));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_send_overhead);
criterion_main!(benches);
