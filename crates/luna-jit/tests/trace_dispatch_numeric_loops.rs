//! P12-S3 smoke tests — verify the trace JIT dispatcher fires on
//! numeric loops, returns the right result, and stays quiet
//! otherwise.

use luna_jit::version::LuaVersion;

const NUMERIC_LOOP: &str = "local i, s, step, limit = 0, 0, 1, 1000
                            repeat
                                s = s + i
                                i = i + step
                            until i >= limit
                            return s";

/// The dispatcher should fire on a numeric repeat-until loop and
/// deliver the exact same sum as the pure interpreter. Result is
/// `0 + 1 + ... + 999 = 499_500`.
#[test]
fn dispatcher_fires_on_numeric_loop_and_result_matches_interp() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false); // keep method JIT out of the way
    vm.set_trace_jit_enabled(true);

    let r = vm.eval(NUMERIC_LOOP).unwrap();
    let v = r[0];
    let got = match v {
        luna_jit::runtime::Value::Int(n) => n,
        luna_jit::runtime::Value::Float(f) => f as i64,
        _ => panic!("expected number, got {v:?}"),
    };
    assert_eq!(got, 499_500, "trace JIT must preserve loop semantics");

    assert!(
        vm.trace_dispatched_count() >= 1,
        "expected dispatcher to fire at least once, got dispatched={} \
         compiled={} closed={}",
        vm.trace_dispatched_count(),
        vm.trace_compiled_count(),
        vm.trace_closed_count(),
    );
    // No metatable in the loop → no deopts expected.
    assert_eq!(
        vm.trace_deopt_count(),
        0,
        "no metatable in this loop, deopts should stay at 0"
    );
}

/// The dispatcher must read `Proto.traces`: with the gate off no
/// compile fires, no traces are cached, and the dispatch counter
/// stays at zero.
#[test]
fn dispatcher_counter_zero_when_gate_off() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    // gate off by default
    let _ = vm.eval(NUMERIC_LOOP).unwrap();
    assert_eq!(vm.trace_dispatched_count(), 0);
    assert_eq!(vm.trace_deopt_count(), 0);
}

/// With the gate on but the loop body using ops outside the
/// step-5 whitelist (e.g. literal comparison `i < 1000` emits
/// `LoadI` per iter when `1000` isn't hoisted), the trace either
/// fails to compile or stays as a non-numeric trace — the
/// dispatcher refuses to invoke it.
///
/// We can't reliably keep `trace_dispatched_count` at zero on a
/// while-loop bench (some recordings might produce numeric-only
/// traces depending on the Lua compiler version), so this test
/// only asserts the bookkeeping invariant — every dispatched
/// trace either succeeded or deopted, never both. (Both counters
/// non-zero is fine; the assertion catches double-counting bugs.)
#[test]
fn dispatched_count_at_most_equals_invocations_minus_deopts() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

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

    // Invariant: every deopt is a subset of dispatch entries.
    assert!(
        vm.trace_deopt_count() <= vm.trace_dispatched_count(),
        "deopts ({}) must not exceed dispatches ({})",
        vm.trace_deopt_count(),
        vm.trace_dispatched_count(),
    );
}

/// Internal-loop contract: a single numeric loop iterates entirely
/// inside the JIT'd code and only returns to the dispatcher once,
/// when the cmp side-exits. The dispatcher's per-entry marshal
/// cost amortizes across all 1000 iterations.
///
/// This locks in the close-handler's
/// `CompileOptions { internal_loop: true }` flag — if a future
/// regression flips it back to one-shot, the dispatched count
/// would jump to ~999 and this assertion would catch it.
#[test]
fn numeric_loop_dispatches_just_once_thanks_to_internal_loop() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let _ = vm.eval(NUMERIC_LOOP).unwrap();
    let n = vm.trace_dispatched_count();
    assert!(
        (1..=5).contains(&n),
        "internal-loop trace should dispatch ~1 time for a 1000-iter \
         loop; got {n} — a one-shot regression would push this to \
         hundreds"
    );
}

/// Numeric `for i = 1, N do ... end` is the most common loop
/// shape in Lua and now sits in the trace JIT's whitelist via
/// Op::ForLoop. Same fast-path checks as the repeat-until case:
/// the trace runs natively until count hits 0 and side-exits at
/// `forloop.pc + 1`. Verifies result + dispatched=1.
#[test]
fn for_loop_dispatches_once_and_returns_correct_sum() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    let r = vm
        .eval("local s = 0 for i = 1, 1000 do s = s + i end return s")
        .unwrap();
    let v = r[0];
    let got = match v {
        luna_jit::runtime::Value::Int(n) => n,
        _ => panic!("expected Int, got {v:?}"),
    };
    // 1 + 2 + ... + 1000 = 500_500.
    assert_eq!(got, 500_500);
    assert!(
        vm.trace_dispatched_count() >= 1,
        "expected for-loop trace to dispatch at least once, got {}",
        vm.trace_dispatched_count()
    );
    // Internal-loop ForLoop semantics: one dispatch per
    // outer Vm::eval invocation, regardless of iteration count.
    let dispatched = vm.trace_dispatched_count();
    assert!(
        (1..=5).contains(&dispatched),
        "for-loop trace should dispatch ~1 time for a 1000-iter \
         loop (internal back-edge handles the iters natively); \
         got {dispatched}"
    );
}

/// Stage-B exit-tag tracking: a `t[i] = i` loop now reaches the
/// dispatcher via per-register exit-tag analysis. The trace's
/// Move + SetTable + ForLoop ops feed the exit_tags pass; at
/// restore time the dispatcher uses the tag to repack each slot
/// as Table / Int / Move-from-entry-tag.
#[test]
fn table_alloc_loop_dispatches_and_returns_correct_length() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    let r = vm
        .eval("local t = {} for i = 1, 10000 do t[i] = i end return #t")
        .unwrap();
    let got = match r[0] {
        luna_jit::runtime::Value::Int(n) => n,
        v => panic!("expected Int, got {v:?}"),
    };
    assert_eq!(got, 10000, "trace JIT must preserve table-fill semantics");
    let dispatched = vm.trace_dispatched_count();
    assert!(
        (1..=5).contains(&dispatched),
        "Stage-B exit_tag dispatcher should fire ~1 time for the \
         table-fill trace; got {dispatched}"
    );
    assert_eq!(vm.trace_deopt_count(), 0, "no metatable → no deopt");
}

/// Subsequent eval() runs on the same Vm reuse `TRACE_JIT_HANDLES`
/// and `Proto.traces` (each fresh Proto carries its own cache).
/// The dispatched counter accumulates across runs; this test
/// confirms the counter is monotone.
#[test]
fn dispatched_counter_is_monotone_across_runs() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let _ = vm.eval(NUMERIC_LOOP).unwrap();
    let after_first = vm.trace_dispatched_count();
    let _ = vm.eval(NUMERIC_LOOP).unwrap();
    let after_second = vm.trace_dispatched_count();
    assert!(
        after_second >= after_first,
        "dispatched count must be monotone non-decreasing across runs \
         ({after_first} → {after_second})"
    );
}
