//! `Proto.has_dispatchable_trace` gates the dispatcher's per-instruction
//! scan of `Proto.traces`. It must be set exactly when some cached trace
//! could be admitted at its head (dispatchable, or linked for
//! down-recursion), or the dispatcher would skip a trace it should enter.

use luna_jit::runtime::{Gc, LuaClosure, Value};
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

fn admissible(cl: Gc<LuaClosure>) -> bool {
    cl.proto
        .traces
        .borrow()
        .iter()
        .any(|t| t.dispatchable || t.downrec_link.is_some())
}

fn run(vm: &mut Vm, src: &[u8]) -> Vec<Value> {
    let cl = vm.load(src, b"=gate").expect("load");
    let mut r = vm.call_value(Value::Closure(cl), &[]).expect("run");
    r.push(Value::Closure(cl));
    r
}

#[test]
fn flag_is_set_for_a_dispatched_loop_trace() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    // the method JIT would take the whole chunk
    vm.set_jit_enabled(false);
    let r = run(
        &mut vm,
        b"local s = 0 for i = 1, 5000 do s = s + i end return s",
    );
    assert!(matches!(r[0], Value::Int(12_502_500)), "{r:?}");
    let Value::Closure(main) = r[1] else { panic!() };
    assert!(admissible(main));
    assert!(main.proto.has_dispatchable_trace.get());
    assert!(vm.trace_dispatched_count() > 0);
}

#[test]
fn flag_stays_clear_when_only_undispatchable_traces_are_cached() {
    // `get` returns a table read with no consumer in the trace: the
    // lowerer cannot type it and keeps the call-triggered trace
    // undispatchable, so `get` ends up with a cached trace and no
    // admissible one
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.open_base();
    let r = run(
        &mut vm,
        b"
        local o = {t = {}}
        local function get(self, k) return self.t[k] end
        local n = 0
        for i = 1, 500 do if get(o, i) == nil then n = n + 1 end end
        return n, get
    ",
    );
    assert!(matches!(r[0], Value::Int(500)), "{r:?}");
    let Value::Closure(get) = r[1] else { panic!() };
    let cached = get.proto.traces.borrow().len();
    assert!(cached > 0, "get compiled no trace");
    assert!(!admissible(get));
    assert!(!get.proto.has_dispatchable_trace.get());
}

#[test]
fn flag_is_set_for_a_trace_with_a_downrec_link() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_self_link_enabled(true);
    vm.open_base();
    let r = run(
        &mut vm,
        b"
        local function fib(n)
            if n < 2 then return n end
            return fib(n - 1) + fib(n - 2)
        end
        local s = 0
        for i = 1, 200 do s = s + fib(3) end
        return s, fib
    ",
    );
    assert!(matches!(r[0], Value::Int(400)), "{r:?}");
    let Value::Closure(fib) = r[1] else { panic!() };
    let traces = fib.proto.traces.borrow();
    assert!(
        traces.iter().any(|t| t.downrec_link.is_some()),
        "no downrec trace was cached"
    );
    assert!(fib.proto.has_dispatchable_trace.get());
    drop(traces);
    assert!(
        vm.trace_downrec_dispatched_count() + vm.trace_downrec_deopt_count() > 0,
        "the downrec trace was never admitted"
    );
}

#[test]
fn flag_is_set_for_an_undispatchable_trace_admitted_by_its_downrec_link() {
    // one recursive call site: the down-recursion guard has a single
    // caller pc, so the trace stays undispatchable and is admitted only
    // through its link
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_self_link_enabled(true);
    vm.open_base();
    let r = run(
        &mut vm,
        b"
        local function sum(n)
            if n < 1 then return 0 end
            return n + sum(n - 1)
        end
        local s = 0
        for i = 1, 300 do s = s + sum(3) end
        return s, sum
    ",
    );
    assert!(matches!(r[0], Value::Int(1800)), "{r:?}");
    let Value::Closure(sum) = r[1] else { panic!() };
    let traces = sum.proto.traces.borrow();
    let shapes: Vec<_> = traces
        .iter()
        .map(|t| {
            (
                t.head_pc,
                t.dispatchable,
                t.downrec_link.is_some(),
                t.dispatch_off_reason,
            )
        })
        .collect();
    assert!(
        traces
            .iter()
            .any(|t| !t.dispatchable && t.downrec_link.is_some()),
        "no undispatchable downrec trace: {shapes:?}"
    );
    assert!(!traces.iter().any(|t| t.dispatchable), "{shapes:?}");
    assert!(sum.proto.has_dispatchable_trace.get());
    drop(traces);
    assert!(
        vm.trace_downrec_dispatched_count() + vm.trace_downrec_deopt_count() > 0,
        "the downrec trace was never admitted"
    );
}

// the call trigger stops looking at a Proto's entry once a trace is cached
// there or recording it was abandoned; `trace_call_head_settled` stands in
// for scanning `traces` on every call

#[test]
fn call_head_settles_when_a_call_trace_is_cached() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.open_base();
    let r = run(
        &mut vm,
        b"
        local o = {t = {}}
        local function get(self, k) return self.t[k] end
        local n = 0
        for i = 1, 500 do if get(o, i) == nil then n = n + 1 end end
        return n, get
    ",
    );
    let Value::Closure(get) = r[1] else { panic!() };
    assert!(get.proto.traces.borrow().iter().any(|t| t.head_pc == 0));
    assert!(get.proto.trace_call_head_settled.get());
}

#[test]
fn call_head_settles_when_recording_it_is_abandoned() {
    // a trace compiler that never succeeds: the head is abandoned after
    // three failed recordings and no later call records it again
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.install_null_jit();
    vm.set_trace_jit_enabled(true);
    let r = run(
        &mut vm,
        b"
        local function f(x) return x + 1 end
        local s = 0
        for i = 1, 20 do s = f(s) end
        return s, f
    ",
    );
    let Value::Closure(f) = r[1] else { panic!() };
    assert!(
        !f.proto.trace_call_head_settled.get(),
        "settled below the threshold"
    );
    let r = run(
        &mut vm,
        b"
        local function f(x) return x + 1 end
        local s = 0
        for i = 1, 1000 do s = f(s) end
        return s, f
    ",
    );
    assert!(matches!(r[0], Value::Int(1000)), "{r:?}");
    let Value::Closure(f) = r[1] else { panic!() };
    assert!(f.proto.trace_call_head_settled.get());
    assert!(f.proto.traces.borrow().is_empty());
    let failed = vm.trace_compile_failed_count();
    // three for f's entry, up to three for the loop's head (a head is
    // recorded again after each failure until it is abandoned)
    assert!((3..=6).contains(&failed), "{failed}");
    for i in 0..200 {
        vm.call_value(Value::Closure(f), &[Value::Int(i)])
            .expect("call f");
    }
    assert_eq!(
        vm.trace_compile_failed_count(),
        failed,
        "recording did not stop"
    );
}

// The interpreter's fast loop only hands an instruction to the dispatcher at
// a pc listed in `Proto::trace_heads`. A loop head reached by falling
// through from the instruction before it (the first arrival, no back edge
// yet) must still be entered: with a one-iteration loop, that arrival is
// the only chance the trace has to run.
#[test]
fn a_head_reached_by_falling_through_is_entered() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    let r = run(
        &mut vm,
        b"
        local function f(n)
            local i = 0
            while i < n do i = i + 1 end
            return i
        end
        return f
    ",
    );
    let Value::Closure(f) = r[0] else {
        panic!("{r:?}")
    };
    let warm = vm
        .call_value(Value::Closure(f), &[Value::Int(1000)])
        .expect("warm");
    assert!(matches!(warm[0], Value::Int(1000)), "{warm:?}");
    let heads = f.proto.trace_heads.get();
    let admissible_heads: Vec<u32> = f
        .proto
        .traces
        .borrow()
        .iter()
        .filter(|t| t.dispatchable || t.downrec_link.is_some())
        .map(|t| t.head_pc)
        .collect();
    assert_eq!(admissible_heads.len(), 1, "{admissible_heads:?}");
    assert_eq!(heads[0], admissible_heads[0]);
    assert_eq!(heads[1], luna_jit::runtime::function::TRACE_HEADS_NONE);
    let before = vm.trace_dispatched_count();
    for _ in 0..100 {
        let r = vm
            .call_value(Value::Closure(f), &[Value::Int(1)])
            .expect("call");
        assert!(matches!(r[0], Value::Int(1)), "{r:?}");
    }
    assert_eq!(vm.trace_dispatched_count() - before, 100);
}
