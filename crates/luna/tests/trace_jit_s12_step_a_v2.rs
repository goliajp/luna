//! P12-S12-A-v2 — Op::TestSet kind-fold. Mirrors S12-A-v1 Op::Test
//! but the source is R[B] (not R[A]) and the on-test-pass branch
//! emits a Move-style def_var R[A] = R[B] with kind propagation.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// `local x = a or b` lowers (when a truthy at trace time) to
/// TestSet that copies a → x on pass. Kind-fold case: a is Int.
#[test]
fn testset_truthy_int_compiles() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local a = i
                 local x = a or 99
                 s = s + x
             end
             return s",
        )
        .unwrap();
    // a = i (always truthy), x = a = i, sum 1..1000 = 500500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(500500)),
        "expected Int(500500), got {:?}",
        r[0]
    );
}

/// `local x = a and b` lowers (when a truthy) to TestSet that
/// proceeds to evaluate b. Kind-fold case.
#[test]
fn testset_and_chain_compiles() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local a = 1
                 local b = i
                 local x = a and b
                 s = s + x
             end
             return s",
        )
        .unwrap();
    // a=1 truthy, x = b = i, sum = 500500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(500500)),
        "expected Int(500500), got {:?}",
        r[0]
    );
}

/// `local x = nil_var or fallback` — falsy source, TestSet fails,
/// pc++ skips the move, fallback is loaded.
#[test]
fn testset_falsy_nil_falls_through_correctly() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local a = nil
                 local x = a or i
                 s = s + x
             end
             return s",
        )
        .unwrap();
    // a nil, x = i, sum = 500500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(500500)),
        "expected Int(500500), got {:?}",
        r[0]
    );
}
