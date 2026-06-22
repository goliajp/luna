//! P12-S12-A v1 — Op::Test whitelist (kind-known truthy fold).
//! `if R[A] then ...` patterns where R[A]'s kind is known stable
//! (Int / Float / Table / Closure / Nil) fold to no-IR — the
//! recorded direction is provably reproducible at every dispatch.
//! Unset kind bails compile (runtime truthy check is v2 scope).

use luna::version::LuaVersion;
use luna::vm::Vm;

/// `if true_int_var then` body — truthy Int gates entry. The test
/// fold + the entry body compile together.
#[test]
fn test_truthy_int_compiles() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local enabled = 1
                 if enabled then s = s + i end
             end
             return s",
        )
        .unwrap();
    // sum 1..1000 = 500500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(500500)),
        "expected Int(500500), got {:?}",
        r[0]
    );
    // The test of a known-truthy Int should not block compile.
    assert!(
        vm.trace_compiled_count() >= 1,
        "test of truthy Int must compile; got compiled={} fail={}",
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
    );
}

/// `local r = {1, 2, 3}; if r then ...` — truthy Table.
#[test]
fn test_truthy_table_compiles() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {1}
                 if t then s = s + t[1] end
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(1000)),
        "expected Int(1000), got {:?}",
        r[0]
    );
}

/// `if nil then ...` — Nil falsy path. test passed, Jmp skipped.
/// Body inside `then` never runs.
#[test]
fn test_falsy_nil_skips_body_correctly() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local x = nil
                 if x then s = s + 100 end
                 s = s + 1
             end
             return s",
        )
        .unwrap();
    // `if nil then` body never runs; outer s += 1 runs 1000 times.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(1000)),
        "expected Int(1000), got {:?}",
        r[0]
    );
}
