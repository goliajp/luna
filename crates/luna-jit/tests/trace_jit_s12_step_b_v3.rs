//! P12-S12-B v3 — ipairs `inext` fast path in `luna_jit_op_tforcall`.
//!
//! v2 (`1e66908`) routed every generic-for iter through `begin_call`
//! → `ipairs_iter` Rust dispatch. v3 detects `R[A]=Native(ipairs_iter)`
//! + `R[A+1]=Table` (no metatable) + `R[A+2]=Int` at the top of
//! `jit_op_tforcall` and short-circuits to `Table::get_int(next_i)`,
//! skipping begin_call / nat_arg / nat_return. Anything else falls
//! through to the v2 generic path, so semantics + tests for non-
//! ipairs traces stay unchanged.

use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// Canonical ipairs over Int array — exercise the fast path.
#[test]
fn ipairs_fast_path_correctness_and_dispatch() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {10, 20, 30, 40, 50}
             local s = 0
             for iter = 1, 200 do
                 for i, v in ipairs(t) do
                     s = s + i + v
                 end
             end
             return s",
        )
        .unwrap();
    // 1+2+3+4+5 = 15; 10+20+30+40+50 = 150; total per outer = 165;
    // 200 outer iters → 33000.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(33000)),
        "expected Int(33000), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fast path must still dispatch generic-for trace; \
         dispatched_count={}",
        vm.trace_dispatched_count(),
    );
}

/// Table with `__index` metatable — fast path must fall through to
/// the v2 generic path so user-defined indexing still works.
#[test]
fn ipairs_with_metatable_falls_through() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // Wrapper table whose __index forwards lookups to `inner`. ipairs
    // iter walks the metatable chain (via index_value) on the wrapper.
    let r = vm
        .eval(
            "local inner = {7, 8, 9}
             local wrapper = setmetatable({}, { __index = inner })
             local s = 0
             for iter = 1, 200 do
                 for _, v in ipairs(wrapper) do
                     s = s + v
                 end
             end
             return s",
        )
        .unwrap();
    // ipairs over wrapper sees the same array part via __index.
    // sum 7+8+9 = 24; 200 * 24 = 4800.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(4800)),
        "expected Int(4800), got {:?}",
        r[0]
    );
}

/// Mixed sparse table — ipairs stops at the first nil, exercising
/// the Nil-return branch of the fast path.
#[test]
fn ipairs_fast_path_stops_at_first_nil() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {100, 200, nil, 999}
             local s = 0
             for iter = 1, 200 do
                 for _, v in ipairs(t) do
                     s = s + v
                 end
             end
             return s",
        )
        .unwrap();
    // ipairs walks 1, 2 then stops at 3 (nil); 100+200 = 300; total
    // 200 * 300 = 60000.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(60000)),
        "expected Int(60000), got {:?}",
        r[0]
    );
}
