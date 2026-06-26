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
