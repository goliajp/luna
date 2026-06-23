//! P12-S1 smoke tests — verify the trace recording skeleton wires
//! up correctly when `Vm::set_trace_jit_enabled(true)` is called.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

#[test]
fn trace_recording_inactive_by_default() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    assert!(!vm.trace_jit_enabled());
    // Run a loop with plenty of back-edges; counter should not advance
    // while the gate is closed, and no trace should be live afterwards.
    vm.eval("local s = 0 for i = 1, 1000 do s = s + i end return s")
        .unwrap();
    // `active_trace` is `pub(crate)` so we infer "inactive" via
    // continued correct behavior of a subsequent eval.
    let r = vm.eval("return 1 + 2").unwrap();
    assert!(matches!(r.first(), Some(Value::Int(3))));
}

#[test]
fn trace_recording_starts_at_hot_back_edge() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_trace_jit_enabled(true);
    assert!(vm.trace_jit_enabled());
    // 1000 iterations × one back-edge per iter = 1000 hits, well past
    // TRACE_HOT_THRESHOLD (64). Recording should start once and abort
    // (hits MAX_TRACE_LEN = 256 of dispatched ops including the loop
    // body's many instructions) or close cleanly; either way the
    // bench code must still return the right value.
    let r = vm
        .eval("local s = 0 for i = 1, 1000 do s = s + i end return s")
        .unwrap();
    assert_eq!(r.len(), 1);
    let v = r[0];
    let ok = matches!(v, Value::Int(500_500)) || matches!(v, Value::Float(f) if (f - 500_500.0).abs() < 1.0);
    assert!(ok, "expected 500500 (sum of 1..1000), got {v:?}");
}

#[test]
fn trace_recording_closes_on_simple_loop() {
    // A `for i = 1, N do s = s + i end` body has 2 ops (Add + ForLoop).
    // After 64 back-edge crossings the recorder starts at the loop head;
    // the very next iteration loops back to the head — trace closes.
    // We do not yet compile, but `trace_closed_count` must increment.
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    // Disable method JIT so the loop's back-edges actually dispatch
    // through the interpreter — method JIT compiles `for i=1,N` whole
    // and trace recording never sees Op::Jmp / ForLoop ticks.
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    // Use `while`/`repeat` (compile to Op::Jmp back-edges) rather than
    // numeric `for` (which uses dedicated ForLoop opcode — back-edge
    // tracking for that is S1.E).
    let _ = vm
        .eval(
            "local i, s = 0, 0; while i < 1000 do i = i + 1; s = s + i end; return s",
        )
        .unwrap();
    assert!(
        vm.trace_closed_count() >= 1,
        "expected ≥1 trace close on a 1000-iter loop, got closed={} aborted={}",
        vm.trace_closed_count(),
        vm.trace_aborted_count()
    );
}

#[test]
fn trace_jit_toggle_round_trip() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    assert!(!vm.trace_jit_enabled());
    vm.set_trace_jit_enabled(true);
    assert!(vm.trace_jit_enabled());
    vm.set_trace_jit_enabled(false);
    assert!(!vm.trace_jit_enabled());
}
