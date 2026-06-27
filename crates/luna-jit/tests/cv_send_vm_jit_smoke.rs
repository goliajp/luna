//! v2.0 Track J sub-step J-E — cross-thread JIT compile + dispatch
//! smoke.
//!
//! This is the regression target the J prep doc §Sub 5 deferred from
//! the J-A landing commit. Now that J-A (`SendJitModule` sleeve),
//! J-B (per-`Vm` JIT storage), and J-D (`scoped_jit_vm_rebind` RAII
//! TLS install/restore) have all landed, the cross-thread JIT story
//! is verifiable end-to-end:
//!
//! 1. Build a JIT-equipped `Vm` on the main thread.
//! 2. Wrap it in [`SendVm::from_vm`] (J-E's new constructor).
//! 3. Move the `SendVm` across a `std::thread::spawn` boundary.
//! 4. On the worker thread, eval a script that engages the trace JIT
//!    (`set_trace_jit_enabled(true)` + a tight hot loop).
//! 5. Assert the result matches the single-thread baseline.
//! 6. Assert `trace_dispatched_count > 0` (i.e. the trace JIT did
//!    actually compile and dispatch on the worker thread; this is
//!    the load-bearing assertion — without it, "cross-thread JIT
//!    works" would silently degrade to "cross-thread interp works,
//!    JIT noop'd").
//!
//! Gated behind `feature = "send"`. Run via:
//!     cargo test -p luna-jit --features send --test cv_send_vm_jit_smoke

#![cfg(feature = "send")]

use std::thread;

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::SendVm;

/// Hot-loop workload that reliably engages the trace JIT. Counted
/// `for` loops compile to a back-edge trace; 1000 iterations is
/// enough to clear the recorder's hot-count threshold and dispatch
/// the compiled trace many times.
///
/// Returns the closed-form sum 1+2+…+1000 = 500500 so the assertion
/// is exact.
const TRACE_HOT_LOOP: &str = r#"
    local s = 0
    for i = 1, 1000 do s = s + i end
    return s
"#;

const EXPECTED_RESULT: i64 = 500_500;

/// Compile-time assertion: `SendVm: Send`. luna-core already pins
/// this in `crates/luna-core/tests/send_vm.rs:send_vm_is_send`, but
/// re-pinning it inside `luna-jit` makes the cross-thread JIT test
/// suite self-contained — if SendVm's Send story ever regresses
/// (e.g. someone adds a `!Send` field without lifting `unsafe impl
/// Send` past it), this binary stops compiling alongside `luna-core`'s
/// pin instead of only the latter.
#[test]
fn send_vm_is_send() {
    const fn require_send<T: Send>() {}
    require_send::<SendVm>();
}

/// Single-thread baseline: build a JIT-equipped Vm, eval the
/// trace-engaging hot loop on the construct thread, capture the
/// result + the trace_dispatched_count. The cross-thread test below
/// asserts identical outcomes after the move.
fn single_thread_baseline() -> (i64, u64) {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    vm.open_base();
    vm.open_math();
    // Disable the chunk-compile JIT so the trace JIT is the sole
    // engine that can lift the result (mirrors the diag_fib28_self_rec
    // Row 2/3 setup); makes the `trace_dispatched_count > 0`
    // assertion below load-bearing rather than masked by the chunk
    // JIT serving the call.
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm.eval(TRACE_HOT_LOOP).expect("baseline eval");
    let n = match r.first() {
        Some(Value::Int(n)) => *n,
        other => panic!("baseline: expected Int, got {:?}", other),
    };
    let dispatched = vm.trace_dispatched_count();
    (n, dispatched)
}

/// Cross-thread JIT smoke: same Vm setup as the baseline, but the
/// `SendVm` wrapper moves to a worker thread and the eval runs there.
///
/// Verifies:
///
/// 1. The `SendVm: Send` impl plus `JitHandle: Send` / `TraceHandle:
///    Send` lifts (J-E Phase B) actually allow the move (compile-time).
/// 2. The wrapped Vm's `JitState` survives the move with its
///    `chunk_compiler` / `trace_compiler` trait objects intact
///    (`Box<dyn Trait>` is `!Send` by default; the SendVm's outer
///    `unsafe impl Send` is what carries the safety claim).
/// 3. On the worker thread, the J-D `scoped_jit_vm_rebind` RAII
///    re-arms the `JIT_VM` / `JIT_CL` TLS slots so the dispatcher
///    can call into JIT helpers.
/// 4. The trace recorder produces a compiled trace on the worker
///    thread's `JITModule` (its `SendJitModule` sleeve) and the
///    handle parks on `Vm.jit.storage.trace_handles` (J-B's
///    migration) so the entry pointer stays callable.
/// 5. `trace_dispatched_count` bumps on the worker thread — the
///    canonical signal that JIT compile + dispatch fired (not just
///    interp).
/// 6. The numeric result is bit-identical to the single-thread
///    baseline.
#[test]
fn jit_equipped_vm_crosses_thread_via_send_vm_and_dispatches_trace() {
    let (baseline_result, baseline_dispatched) = single_thread_baseline();
    assert_eq!(
        baseline_result, EXPECTED_RESULT,
        "single-thread baseline produced wrong result"
    );
    assert!(
        baseline_dispatched > 0,
        "single-thread baseline must dispatch the trace at least once; \
         got trace_dispatched_count={}",
        baseline_dispatched
    );

    // Build a JIT-equipped Vm on the main thread; wrap in SendVm.
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    vm.open_base();
    vm.open_math();
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    let send = SendVm::from_vm(vm);

    // Ship across the thread boundary. The `move` closure forces
    // `SendVm: Send` to be load-bearing at compile time; the runtime
    // ownership transfer then exercises the actual cross-thread
    // sleeve + RAII machinery.
    let handle = thread::spawn(move || {
        // On the worker thread now. Drive the JIT through SendVm's
        // lock-protected `eval`.
        let r = send.eval(TRACE_HOT_LOOP).expect("worker eval");
        let n = match r.first() {
            Some(Value::Int(n)) => *n,
            other => panic!("worker: expected Int, got {:?}", other),
        };
        let dispatched = send.trace_dispatched_count();
        (n, dispatched)
    });

    let (worker_result, worker_dispatched) = handle.join().expect("worker thread panicked");

    assert_eq!(
        worker_result, baseline_result,
        "cross-thread JIT result must match single-thread baseline; \
         got worker={}, baseline={}",
        worker_result, baseline_result
    );
    assert!(
        worker_dispatched > 0,
        "cross-thread JIT must dispatch the trace at least once on \
         the worker thread; got trace_dispatched_count={} (would \
         silently regress to interp-only otherwise)",
        worker_dispatched
    );
}

/// Compile the trace JIT on the main thread first, THEN move the
/// SendVm to a worker thread and re-eval the SAME script. This is
/// the strictest cross-thread invariant test: the compiled trace's
/// mcode (a `*const u8` raw pointer into a mmap'd page owned by the
/// `SendJitModule`) MUST remain dispatchable from the worker thread
/// after the move, because the parent module ships with the handle
/// inside `Vm.jit.storage.trace_handles` (J-B's migration). If
/// `entry_raw` had any per-thread liveness assumption, this test
/// would either crash or noop the dispatch.
#[test]
fn trace_compiled_on_thread_a_remains_dispatchable_on_thread_b() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    vm.open_base();
    vm.open_math();
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // Warm the trace JIT on the main thread first. The recorder
    // compiles a trace for the hot loop; `trace_handles` parks the
    // resulting `TraceHandle` (J-B).
    let r0 = vm.eval(TRACE_HOT_LOOP).expect("main-thread warm eval");
    assert!(matches!(r0.first(), Some(Value::Int(EXPECTED_RESULT))));
    let warm_dispatched = vm.trace_dispatched_count();
    assert!(
        warm_dispatched > 0,
        "warm-up must dispatch the trace; got {}",
        warm_dispatched
    );

    // Wrap and ship. The wrapped Vm carries the already-compiled
    // trace in `storage.trace_handles`; the mcode pages travel with
    // the `SendJitModule`.
    let send = SendVm::from_vm(vm);
    let handle = thread::spawn(move || {
        // Re-eval the same script. The trace JIT cache hit should
        // dispatch the previously-compiled trace.
        let r = send.eval(TRACE_HOT_LOOP).expect("worker re-eval");
        let n = match r.first() {
            Some(Value::Int(n)) => *n,
            other => panic!("worker: expected Int, got {:?}", other),
        };
        let dispatched = send.trace_dispatched_count();
        (n, dispatched)
    });

    let (worker_result, worker_dispatched) = handle.join().expect("worker thread panicked");
    assert_eq!(
        worker_result, EXPECTED_RESULT,
        "cross-thread re-dispatch result"
    );
    assert!(
        worker_dispatched > warm_dispatched,
        "worker eval must add to the dispatch counter (re-running the \
         hot loop should re-dispatch the warmed trace); got \
         worker_dispatched={}, warm_dispatched={}",
        worker_dispatched,
        warm_dispatched
    );
}

/// v2.1 Track J-C Phase D — IR-aware cross-thread smoke.
///
/// J-C migrates the trace IR types (`CompiledTrace`,
/// `Proto::traces`, `InlineSideExit`, …) to cfg-gated Send wrappers
/// (`TArc` / `TCellU32` / `TCellBool` / `TCellPtr` / `TRefLock`) so
/// under `feature = "send"` the IR is *structurally* Send + Sync
/// — no `unsafe impl Send` lifted past the trace types themselves.
///
/// This test complements J-E's `trace_compiled_on_thread_a_remains_
/// dispatchable_on_thread_b` by pinning the J-C invariants:
///
/// 1. **Structural Send + Sync** of `CompiledTrace` and `Proto` at
///    the type system layer (compile-time `require_send_sync`
///    helpers). Under `feature = "send"` this is the load-bearing
///    claim — the SendVm move would be unsound without it.
/// 2. **Cross-thread `trace_dispatched_count` growth** — same
///    angle as J-E's smoke but with a longer scaled-up loop so the
///    `TCellU32` (under send: `AtomicU32`) cells inside the IR see
///    many writes during the worker eval. Proves the cell-API
///    swap (`Cell::get/set` → `Atomic::load/store(Relaxed)`) reads
///    + writes the same observable u32 across the thread move.
///
/// If a future Cranelift bump or an unrelated change regresses the
/// J-C invariants (e.g. a non-Send field sneaks into the IR), the
/// `require_send_sync` static assertion fails to compile — earlier
/// signal than any runtime regression.
#[test]
fn jit_aware_send_vm_ir_walks_cross_thread() {
    use luna_jit::jit::trace::CompiledTrace;
    use luna_jit::runtime::function::Proto;

    // Static assertions — these are the J-C structural claims.
    //
    // CompiledTrace: every interior-mutability field flips to a
    // Send + Sync wrapper under `feature = "send"` (J-C Phase B), so
    // the whole struct becomes structurally Send + Sync — no
    // `unsafe impl Send for CompiledTrace` needed.
    const fn require_send_sync<T: Send + Sync>() {}
    require_send_sync::<CompiledTrace>();
    // `Proto` itself stays !Send + !Sync because it holds `Box<[Value]>`
    // and Value embeds `Gc<T>` = `NonNull<T>` which is unconditionally
    // !Send + !Sync. The SendVm wrapper's outer `unsafe impl Send`
    // carries that around; J-C is explicitly the trace-IR scope, not
    // the GC migration (that's a separate v2+ track). Assert only the
    // `traces` field wrapper instead:
    const fn require_send<T: Send>() {}
    require_send::<
        luna_jit::jit::send_compat::TRefLock<
            Vec<luna_jit::jit::send_compat::TArc<CompiledTrace>>,
        >,
    >();
    // Probe that we can actually name `Proto` (otherwise the dead
    // import lint would fire).
    let _: Option<&Proto> = None;

    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    vm.open_base();
    vm.open_math();
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // Warm: compile the trace on the main thread.
    let r0 = vm.eval(TRACE_HOT_LOOP).expect("main-thread warm eval");
    assert!(matches!(r0.first(), Some(Value::Int(EXPECTED_RESULT))));
    let warm_dispatched = vm.trace_dispatched_count();
    assert!(warm_dispatched > 0, "warm-up must dispatch the trace");

    let send = SendVm::from_vm(vm);
    let baseline_dispatched = send.trace_dispatched_count();

    // Ship to the worker thread and re-dispatch. The IR (parent
    // trace's `CompiledTrace`, `exit_hit_counts` cells, etc.)
    // survives the move because every interior-mutability slot is
    // now backed by a Send + Sync wrapper under `feature = "send"`.
    let handle = thread::spawn(move || {
        // Re-eval N=4 times so the worker thread definitely bumps
        // the dispatch counter (and incidentally walks the IR's
        // exit_hit_counts cells N times each iteration).
        let mut last_n: i64 = 0;
        for _ in 0..4 {
            let r = send.eval(TRACE_HOT_LOOP).expect("worker eval");
            last_n = match r.first() {
                Some(Value::Int(n)) => *n,
                other => panic!("worker: expected Int, got {:?}", other),
            };
        }
        let worker_dispatched = send.trace_dispatched_count();
        (last_n, worker_dispatched)
    });
    let (worker_result, worker_dispatched) = handle.join().expect("worker panicked");

    assert_eq!(worker_result, EXPECTED_RESULT);
    // Worker thread sees the same baseline trace_dispatched_count
    // before its evals, and grows it past baseline after.
    assert!(
        worker_dispatched > baseline_dispatched,
        "worker eval must increment trace_dispatched_count past the \
         pre-move baseline (proves the trace IR survived the move \
         and the dispatcher fired on the worker); baseline={} \
         worker={}",
        baseline_dispatched,
        worker_dispatched
    );
}

/// v2.0 Track J sub-step J-E Phase E — wallclock perf parity bench.
///
/// Charter target: cross-thread JIT eval must stay within 5% of
/// single-thread JIT eval. This measures the steady-state shape
/// (one long-lived worker thread + N evals on it, joined once at
/// the end) — NOT the spawn-per-iter shape. Rationale: the
/// spawn-per-iter shape pays a fixed ~30-100µs thread::spawn cost
/// every iteration, which is OS overhead, not J-E machinery
/// overhead. The 5% gate is specifically pinning the cost of the
/// J-A `SendJitModule` sleeve + J-B per-Vm storage + J-D RAII
/// rebind, NOT the OS thread startup cost.
///
/// Comparison structure:
///   (a) Single-thread: warm the SendVm, run N evals on the
///       construct thread, wallclock the inner loop.
///   (b) Cross-thread steady-state: warm the SendVm, ship to a
///       worker thread, run N evals on the worker, join once,
///       wallclock the inner loop only (excluding spawn/join
///       fixed cost via worker-side timing).
///
/// Marked `#[ignore]` so it doesn't run in the default `cargo
/// test` loop (scheduling variance would make the gate flaky).
/// Run explicitly:
/// `cargo test -p luna-jit --features send --test
/// cv_send_vm_jit_smoke --release -- --ignored --nocapture`.
///
/// IF the wallclock delta blows the 5% gate, the answer is NOT
/// "loosen the gate" — that's the `exec/no-shrink-words` reflex.
/// The right escalation is to instrument which specific cross-
/// thread machinery cost grew. J-D's per-dispatch TLS install +
/// restore is the prime suspect (`scoped_rebind.rs` projected
/// ~5-10 cycles/dispatch).
#[test]
#[ignore]
fn perf_parity_single_thread_vs_cross_thread() {
    use std::time::Instant;

    // N=200 iters at ~55µs/iter = ~11ms per measurement, above
    // macOS QoS scheduling granularity (~1ms). One measurement's
    // variance band is ~3-5% from observed runs; we take MEDIAN of
    // K=5 measurements to reject the occasional outlier where the
    // worker gets scheduled on an efficiency core.
    //
    // Median-of-5 reduces single-outlier sensitivity to the 4 normal
    // observations, while still completing in ~600ms total
    // (5 × 110ms).
    const N: usize = 200;
    const K: usize = 5;

    fn fresh_jit_send_vm() -> SendVm {
        let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
        vm.open_base();
        vm.open_math();
        vm.set_jit_enabled(false);
        vm.set_trace_jit_enabled(true);
        SendVm::from_vm(vm)
    }

    fn measure_single_thread_ns(send: &SendVm) -> f64 {
        let t0 = Instant::now();
        for _ in 0..N {
            let r = send.eval(TRACE_HOT_LOOP).expect("single-thread iter");
            assert!(matches!(r.first(), Some(Value::Int(EXPECTED_RESULT))));
        }
        t0.elapsed().as_nanos() as f64 / N as f64
    }

    // ── Pair A: single-thread baseline ────────────────────────────
    let send_st = fresh_jit_send_vm();
    // Warm-up: first eval compiles the trace; subsequent evals hit
    // the cached trace dispatch.
    let _ = send_st.eval(TRACE_HOT_LOOP).expect("warm-up single");

    let mut single_thread_samples: Vec<f64> =
        (0..K).map(|_| measure_single_thread_ns(&send_st)).collect();
    single_thread_samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let single_thread_ns = single_thread_samples[K / 2];

    // ── Pair B: cross-thread steady-state ─────────────────────────
    // Build a fresh JIT-equipped Vm + ship it via SendVm to ONE
    // worker thread. Worker does its own warm-up then runs K
    // measurements of N iters each, reports the median for the
    // inner loop ONLY (so OS thread spawn/join cost stays outside).
    let send_ct = fresh_jit_send_vm();
    let handle = thread::spawn(move || {
        // Worker-thread warm-up (compiles trace on this OS thread).
        let _ = send_ct.eval(TRACE_HOT_LOOP).expect("warm-up cross");
        let mut samples: Vec<f64> = (0..K)
            .map(|_| {
                let t0 = Instant::now();
                for _ in 0..N {
                    let r = send_ct.eval(TRACE_HOT_LOOP).expect("cross-thread iter");
                    assert!(matches!(r.first(), Some(Value::Int(EXPECTED_RESULT))));
                }
                t0.elapsed().as_nanos() as f64 / N as f64
            })
            .collect();
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        samples[K / 2]
    });
    let cross_thread_ns = handle.join().expect("worker thread panicked");

    let delta_pct = (cross_thread_ns - single_thread_ns) / single_thread_ns * 100.0;
    println!(
        "\nJ-E perf parity (N={} iters/sample, K={} samples, median):\n  {:40} = {:>10.0} ns/iter\n  {:40} = {:>10.0} ns/iter\n  delta = {:+.2}%",
        N,
        K,
        "single-thread (no transfer)",
        single_thread_ns,
        "cross-thread (steady-state worker)",
        cross_thread_ns,
        delta_pct,
    );

    // 5% gate from the J-E charter. This measures the steady-state
    // J-E machinery overhead (sleeve + RAII + per-Vm storage) and
    // excludes OS thread startup cost. Median-of-K rejects single
    // OS-scheduling outliers without loosening the actual gate.
    let gate_pct = 5.0;
    assert!(
        delta_pct.abs() <= gate_pct,
        "cross-thread perf parity blew the J-E charter gate of \
         ±{:.1}%: single-thread (median) = {:.0}ns/iter, cross-thread \
         (median) = {:.0}ns/iter, delta = {:+.2}% — investigate J-D \
         RAII or SendJitModule sleeve regressions; DO NOT loosen \
         the gate (exec/no-shrink-words reflex)",
        gate_pct,
        single_thread_ns,
        cross_thread_ns,
        delta_pct,
    );
}
