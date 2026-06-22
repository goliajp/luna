//! P12-S9-C — emit Op::SetList B=0 using the recorder's var_count
//! snapshot as the effective B. Both the sunk path (def_var virt
//! slots) and the helper path (per-source emit_table_set) honor
//! the snapshot length.
//!
//! Together with S9-B (Call C=0 var_count==1 accept), this opens
//! the Lua frontend pattern
//!     local t = {f(), g()}
//! when g returns 1 value (Call C=0 → top=A+1) and SetList B=0 →
//! effective B = top-A-1. binary_trees `make`'s
//! `return {make(d-1), make(d-1)}` uses exactly this shape.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// `local t = {a, f()}` where f returns 1 value — Lua frontend
/// emits Call C=0 + SetList B=0 (the LAST arg uses variable form).
/// Post-S9-B/C, the SetList B=0 + Call C=0 var_count=1 combo
/// matches cap=2 (a + f() = 2 source slots) → sunk emit.
#[test]
fn setlist_b_zero_with_call_c_zero_sunk_emits() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function one() return 1 end
             local s = 0
             for i = 1, 1000 do
                 local t = {i, one()}
                 s = s + t[1] + t[2]
             end
             return s",
        )
        .unwrap();
    // (i + 1) for i=1..1000 = 500500 + 1000 = 501500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(501500)),
        "expected Int(501500), got {:?}",
        r[0]
    );
    // Compile-success contract: the trace body MUST compile post-
    // S9-B/C (Call C=0 var_count=1 accepted; SetList B=0 uses
    // var_count as effective B and matches cap=2 → sunk).
    assert!(
        vm.trace_compiled_count() >= 1,
        "trace must compile; got closed={} compiled={} fail={}",
        vm.trace_closed_count(),
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
    );
}

/// `local t = {f()}` where f returns 1 value — SetList B=0 with
/// effective B = 1, matches cap=1, sunk emit.
#[test]
fn setlist_b_zero_single_slot_sunk_emits() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function one(x) return x * x end
             local s = 0
             for i = 1, 1000 do
                 local t = {one(i)}
                 s = s + t[1]
             end
             return s",
        )
        .unwrap();
    // sum i^2 for i=1..1000 = 333833500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(333833500)),
        "expected Int(333833500), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_compiled_count() >= 1,
        "trace must compile; got compiled={}, fail={}",
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
    );
}

/// Binary-trees pattern correctness: even though the recursive
/// trace may not dispatch (other gates), `make` produces the right
/// tree shape under interp + leaf trace dispatches. The contract:
/// no compile crash, correct result.
#[test]
fn binary_trees_pattern_correct_under_trace_jit() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function make(d)
                 if d == 0 then return {nil, nil} end
                 return {make(d-1), make(d-1)}
             end
             local function check(t)
                 if t[1] == nil then return 1 end
                 return 1 + check(t[1]) + check(t[2])
             end
             local sum = 0
             for i = 1, 10 do sum = sum + check(make(10)) end
             return sum",
        )
        .unwrap();
    // 10 * (2^11 - 1) = 10 * 2047 = 20470.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(20470)),
        "expected Int(20470), got {:?}",
        r[0]
    );
}
