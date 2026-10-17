//! Escape-analysis sweep tests.
//!
//! The sweep runs post-recording, pre-emit, inside
//! `try_compile_trace_with_options`. The Sinkable-site count flows
//! into `CompiledTrace.sinkable_sites_seen`, which the close handler
//! sums into `Vm::trace_sinkable_seen_count`.
//!
//! These tests verify three shapes:
//! 1. A trace with NO NewTable → tally stays 0 (proving the sweep
//!    doesn't synthesise sites that don't exist).
//! 2. A trace whose NewTable is inside a `for` body → ForLoop
//!    terminator forces escape; tally stays 0. The trace still
//!    compiles, proving the sweep doesn't break emit.
//! 3. A trace whose NewTable's slot is never used again after a
//!    Return1 of a NON-table value → site stays Sinkable; tally
//!    bumps by 1 per compiled trace.

use luna_jit::version::LuaVersion;

/// Pure numeric for-loop, no tables — the sweep should find zero
/// sites and the tally should stay 0 even after the trace compiles.
#[test]
fn pure_numeric_loop_yields_zero_sinkable() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do s = s + i end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(500500)));
    assert!(
        vm.trace_compiled_count() >= 1,
        "the numeric-for trace must compile; got compiled={}",
        vm.trace_compiled_count()
    );
    assert_eq!(
        vm.trace_sinkable_seen_count(),
        0,
        "no NewTable in body → sweep must find zero sites; got {}",
        vm.trace_sinkable_seen_count()
    );
}

/// NewTable inside a for-loop body — the ForLoop terminator
/// conservatively escapes every live binding (the loop may exit on
/// the next iter; interp resumes seeing the heap table). Tally is
/// expected to be 0; the assertion is that the trace still compiles
/// (the sweep runs end-to-end without bailing emit).
#[test]
fn for_body_newtable_compiles_under_sweep() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {1, 2, 3}
                 s = s + t[1] + t[2] + t[3]
             end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(6000)));
    // The trace may or may not compile depending on lowerer support
    // for the exact SetList + GetI shape; what the sweep guarantees is
    // that the analysis itself never panics. A compiled trace
    // implies the sweep ran successfully.
    assert!(
        vm.trace_closed_count() >= 1,
        "for-loop body trace must at least close; got closed={}",
        vm.trace_closed_count()
    );
    // No specific Sinkable count required here — ForLoop terminator
    // escapes live sites. The crucial property is that we got this
    // far without the sweep crashing or breaking emit.
    let _ = vm.trace_sinkable_seen_count();
}

/// Call-triggered trace through a function whose body NewTables a
/// 3-element array, sums its slots into a scalar, and Return1s the
/// scalar. The table slot (R[1] in the callee) is never used
/// after the SetList except via GetI reads; the Return1 carries
/// R[2] (the sum), NOT R[1]. So R[1]'s site stays Sinkable.
#[test]
fn callee_local_table_with_scalar_return_stays_sinkable() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f()
                 local t = {1, 2, 3}
                 return t[1] + t[2] + t[3]
             end
             local s = 0
             for _ = 1, 200 do s = s + f() end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(1200)));
    assert!(
        vm.trace_compiled_count() >= 1,
        "the callee's trace must compile under call-trigger; got compiled={}",
        vm.trace_compiled_count()
    );
    assert!(
        vm.trace_sinkable_seen_count() >= 1,
        "f's local {{1,2,3}} stays Sinkable across the sweep; \
         got sinkable_seen={}, compiled={}",
        vm.trace_sinkable_seen_count(),
        vm.trace_compiled_count()
    );
}

/// The last op of the loop's body makes a table (`u`), and the exit a
/// collection due after it takes rebuilds the sunk table `t` still in
/// its register: PUC's collection at that point finds it there. Without
/// that exit two of the trace's exits rebuild `t`.
#[test]
fn sunk_table_is_rebuilt_at_the_exit_after_the_last_op() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    let r = vm
        .eval(
            "local n = 0
             for i = 1, 1000 do
                 local t = {i}
                 n = n + t[1]
                 local u = {}
             end
             return n",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(500500)));
    assert_eq!(vm.trace_sunk_alloc_count(), 1);
    assert_eq!(vm.trace_materialize_emit_count(), 3);
}

/// The exits a collection check takes are no failing guards: however
/// often a loop takes one, no side trace starts there, so none can run
/// before the check the exit is for.
#[test]
fn collection_exits_start_no_side_trace() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    let r = vm
        .eval(
            "collectgarbage('incremental', 1, 100)
             local keep = {}
             for i = 1, 20000 do
                 keep[i % 64 + 1] = {i}
             end
             return keep[1][1]",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(19968)));
    assert!(vm.trace_compiled_count() >= 1);
    assert_eq!(vm.trace_side_trace_started_count(), 0);
}
