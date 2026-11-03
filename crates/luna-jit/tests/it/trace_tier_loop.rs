//! A trace moving to Cranelift goes on in Cranelift's code of its loop
//! from the baseline code's back edge, carrying the registers over
//! `reg_state`. Cranelift's code keeps no iteration count: once the move
//! is made, the trace's count stays one short of `tier_up_at` however many
//! passes the loop then runs.

use luna_jit::__private::ProtoTraceInternals;
use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

use super::trace_corpus::*;

const AT: u32 = 1000;

/// The closure the chunk `src` returns, in `vm`.
fn function(vm: &mut Vm, src: &str) -> Value {
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    vm.call_value(Value::Closure(main), &[]).expect("chunk")[0]
}

/// Calls `f` in `vm` and the interpreter's copy `g` in `it` with `args`,
/// and checks they return the same.
fn same(vm: &mut Vm, f: Value, it: &mut Vm, g: Value, args: &[Value]) {
    let a = vm.call_value(f, args).expect("trace run");
    let b = it.call_value(g, args).expect("interpreter run");
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}

/// The iteration counts of the traces of `f` that moved on at their loop.
fn moved_counts(f: Value) -> Vec<u32> {
    let Value::Closure(c) = f else {
        panic!("not a function")
    };
    c.proto
        .traces()
        .iter()
        .filter_map(|t| t.tier_up())
        .filter(|t| !luna_jit::__private::tier_up_loop_cell(t).get().is_null())
        .map(|t| t.count().get())
        .collect()
}

const LONG_LOOP: &str = r#"
return function(n)
    local s, f, k, t = 0, 0.5, "", {x = 0}
    for i = 1, n do
        s = s + i % 7
        f = f + 0.25
        t.x = t.x + 1
        if i % 1000 == 0 then k = k .. "." end
    end
    return s, f, #k, t.x, math.type(s) == "integer", math.type(f) == "float"
end
"#;

/// One call of a loop running long past `tier_up_at` moves the loop and
/// runs the rest of it in Cranelift's code, the registers' kinds (integer,
/// float, string, table) as the baseline code left them.
#[test]
fn a_long_loop_goes_on_in_the_optimizing_tier_within_one_call() {
    let mut vm = tiered(LuaVersion::Lua54, 8, TraceTier::Auto, AT);
    let mut it = interp(LuaVersion::Lua54);
    let (f, g) = (function(&mut vm, LONG_LOOP), function(&mut it, LONG_LOOP));
    same(&mut vm, f, &mut it, g, &[Value::Int(50_000)]);
    assert_eq!(vm.trace_tiered_up_count(), 1);
    assert_eq!(
        moved_counts(f),
        [AT - 1],
        "the loop's passes after the move were counted"
    );
}

/// Every later entry runs its first pass in the baseline code and moves on
/// at its first back edge.
#[test]
fn each_later_entry_moves_on_at_its_first_back_edge() {
    let mut vm = tiered(LuaVersion::Lua54, 8, TraceTier::Auto, AT);
    let mut it = interp(LuaVersion::Lua54);
    let (f, g) = (function(&mut vm, LONG_LOOP), function(&mut it, LONG_LOOP));
    for n in [5000, 3, 2, 700, 9000] {
        same(&mut vm, f, &mut it, g, &[Value::Int(n)]);
        assert_eq!(moved_counts(f), [AT - 1], "n = {n}");
    }
}

const SIDE_LOOP: &str = r#"
local function g(x)
    if x % 4 == 0 then return x * 2 end
    return x
end
return function(n)
    local s = 0
    for i = 1, n do s = s + g(i) end
    return s
end
"#;

/// A side trace attached to an exit of the loop (inside the function it
/// inlines) runs from that exit in both tiers' code: once the loop has
/// moved, nothing records that way again.
#[test]
fn a_side_trace_runs_from_both_tiers_code() {
    for v in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = tiered(v, 8, TraceTier::Auto, AT);
        let mut it = interp(v);
        let (f, g) = (function(&mut vm, SIDE_LOOP), function(&mut it, SIDE_LOOP));
        same(&mut vm, f, &mut it, g, &[Value::Int(20_000)]);
        let sides = vm.trace_side_trace_compiled_count();
        assert!(sides > 0, "{v:?}: no side trace");
        let started = vm.trace_side_trace_started_count();
        same(&mut vm, f, &mut it, g, &[Value::Int(20_000)]);
        assert_eq!(moved_counts(f), [AT - 1], "{v:?}");
        assert_eq!(vm.trace_side_trace_started_count(), started, "{v:?}");
        assert_eq!(vm.trace_side_trace_compiled_count(), sides, "{v:?}");
    }
}
