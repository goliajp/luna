//! P12-S2.C smoke tests — verify the close handler wires
//! `try_compile_trace` into `Vm::run` and updates the
//! `trace_compiled_count` / `trace_compile_failed_count` counters.
//!
//! The S3 dispatcher (next phase) reads `Proto.traces`, so these
//! tests only assert that *something* lands there — they don't
//! invoke the compiled trace. That stays the trace JIT contract
//! until S3.

use luna_jit::version::LuaVersion;

/// Every closed trace must move into one of three buckets:
/// (1) dedup-skip (head_pc already in `Proto.traces`),
/// (2) compiled and parked,
/// (3) compile-failed and dropped.
/// So `compiled + failed <= closed`.
#[test]
fn closed_traces_split_across_compiled_and_failed_buckets() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false); // keep method JIT out of the way
    vm.set_trace_jit_enabled(true);

    // Run a loop that drives the back-edge counter past
    // TRACE_HOT_THRESHOLD several times so traces close.
    let _ = vm
        .eval(
            "local i, s = 0, 0
             while i < 1000 do
                 i = i + 1
                 s = s + i
             end
             return s",
        )
        .unwrap();

    let closed = vm.trace_closed_count();
    let compiled = vm.trace_compiled_count();
    let failed = vm.trace_compile_failed_count();
    assert!(
        compiled + failed <= closed,
        "compiled ({compiled}) + failed ({failed}) must not exceed \
         closed ({closed}) — extra closes can be deduped by head_pc"
    );
    // The recorder should have closed at least one trace on a
    // 1000-iter loop; whether the lowerer accepts it depends on
    // the body's op shape (LoadI for the literal 1000 keeps step
    // 5 from accepting most while-loops — see the test below).
    assert!(closed >= 1, "expected ≥1 close, got 0");
}

/// A repeat-until pattern using only step-5-whitelisted ops
/// (Add/Lt/Jmp on locals) closes a trace the lowerer can compile.
/// We can't see `Proto.traces` from the outside cleanly, but
/// `trace_compiled_count` ticking up proves the wiring lands.
#[test]
fn step5_compatible_repeat_until_loop_compiles() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // `step`, `limit`, and `zero`-equivalent literals are all
    // hoisted to locals so the per-iter body never sees LoadI.
    // `repeat ... until i >= limit` puts the exit-cmp at the
    // bottom of the loop — the recorded direction is
    // "matches K=1 → take back-edge Jmp", which is the only
    // direction step 3 accepts.
    let r = vm
        .eval(
            "local i, s, step, limit = 0, 0, 1, 1000
             repeat
                 s = s + i
                 i = i + step
             until i >= limit
             return s",
        )
        .unwrap();
    // 0 + 1 + ... + 999 = 999 * 1000 / 2 = 499_500.
    let v = r[0];
    let got = match v {
        luna_jit::runtime::Value::Int(n) => n,
        luna_jit::runtime::Value::Float(f) => f as i64,
        _ => panic!("expected number, got {v:?}"),
    };
    assert_eq!(got, 499_500);

    assert!(
        vm.trace_compiled_count() >= 1,
        "expected ≥1 step-5-compatible trace to compile, got \
         compiled={} failed={} closed={}",
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
        vm.trace_closed_count(),
    );
}

/// Default behavior unchanged: with the trace JIT gate off, no
/// close handler ever fires, and neither counter moves.
#[test]
fn trace_jit_disabled_keeps_compile_counters_at_zero() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    // gate stays off (default)
    let _ = vm
        .eval(
            "local i, s = 0, 0
             while i < 1000 do
                 i = i + 1
                 s = s + i
             end
             return s",
        )
        .unwrap();

    assert_eq!(vm.trace_closed_count(), 0);
    assert_eq!(vm.trace_compiled_count(), 0);
    assert_eq!(vm.trace_compile_failed_count(), 0);
}

/// Re-running the same loop body shouldn't keep compiling new
/// traces — the hot counter caps at `u32::MAX / 2`, and S2.C's
/// explicit dedup on `head_pc` prevents the same Proto from
/// stacking duplicate traces in `Proto.traces`.
#[test]
fn trace_compile_dedups_by_head_pc_across_reruns() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let src = "local i, s, step, limit = 0, 0, 1, 1000
               repeat
                   s = s + i
                   i = i + step
               until i >= limit
               return s";

    // First run lands the trace.
    let _ = vm.eval(src).unwrap();
    let compiled_after_first = vm.trace_compiled_count();
    // Each `vm.eval(src)` compiles a fresh Proto, so the dedup
    // gate is per-Proto. Within one eval the same loop body is
    // a single Proto — repeated back-edge crossings can't grow
    // `trace_compiled_count` beyond the initial close + compile
    // for that Proto, because the second close hits the dedup
    // path.
    assert!(
        compiled_after_first >= 1,
        "expected ≥1 compile from the first eval, got {compiled_after_first}"
    );

    // Sanity: the cap on re-records (hot counter) plus dedup means
    // no runaway growth. Even after re-eval (which makes a fresh
    // Proto and gets its own trace), the counters move by ≤ 1 per
    // re-eval, not per iter.
    let _ = vm.eval(src).unwrap();
    let compiled_after_second = vm.trace_compiled_count();
    // Allow it to be the same (dedup at a higher level) or to
    // grow by 1 (fresh Proto). Disallow runaway growth.
    assert!(
        compiled_after_second <= compiled_after_first + 1,
        "second eval should add at most one trace, went from \
         {compiled_after_first} to {compiled_after_second}"
    );
}
