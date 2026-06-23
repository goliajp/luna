//! P12-S12-C v1 — `Op::Concat` trace JIT whitelist + helper.
//!
//! Pre-S12-C, any hot loop body with `s = s .. v` (string concat)
//! bailed compile because `Op::Concat` wasn't in
//! `is_whitelisted_step5`. v1 adds the white-listing + a thin
//! helper (`luna_jit_op_concat`) that runs `concat_run` on the
//! same vm.stack the interp would touch + detects/deopts on the
//! `__concat` metamethod path (frame-push detection).
//!
//! **v1 known limitation**: Concat operands whose `current_kinds`
//! is `Unset` (typically Str slots — `RegKind` has no Str variant
//! today) get spilled via `luna_jit_stack_update_raw` which
//! *preserves* vm.stack's existing tag. When the slot's prior
//! interp-written tag matches (Str), correctness holds; when the
//! slot was uninitialised (Nil) or held a stale type from earlier
//! ops, the helper sees the wrong Value type and either deopts or
//! produces wrong output. The v1 tests use Int/Float Concat
//! operands (compiler-known kinds) where this isn't an issue;
//! mixed Str+Unset cases (e.g. `s = s .. v` in `for _,v in
//! ipairs(t)` where v is a Str) wait for v2 to add `RegKind::Str`.
//!
//! No perf claim — helper walks the same algorithm as interp.
//! Real perf wins are P14 (string subsystem rope/builder model).

use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// Concat with two Int operands derived from arith — both
/// `current_kinds` slots are `Int` so spill uses the proper
/// `spill_to_stack` (tag=INT) path, not `update_raw`.
#[test]
fn concat_int_operands_compiles_and_dispatches() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function build(n)
                 local last = ''
                 for i = 1, n do
                     local x = i + 0
                     local y = i + 1
                     last = x .. y
                 end
                 return last
             end
             return build(200)",
        )
        .unwrap();
    // last iter: i=200, x=200, y=201. concat → '200201'.
    match r[0] {
        luna_jit::runtime::Value::Str(ref s) => {
            assert_eq!(
                s.as_bytes(),
                b"200201",
                "expected '200201', got {:?}",
                s.as_bytes()
            );
        }
        ref other => panic!("expected Str, got {:?}", other),
    }
    assert!(
        vm.trace_compiled_count() >= 1,
        "expected at least one trace to compile; \
         compiled_count={}",
        vm.trace_compiled_count(),
    );
}

/// Mixed Int + Float operands — Concat with one Int and one
/// Float slot. Tests that both `RegKind::Int` and `RegKind::Float`
/// spill paths emit the correct tag bytes.
#[test]
fn concat_int_float_correctness() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function build(n)
                 local last = ''
                 for i = 1, n do
                     local a = i * 2     -- Int
                     local b = i + 0.5   -- Float
                     last = a .. b
                 end
                 return last
             end
             return build(200)",
        )
        .unwrap();
    // last iter: i=200, a=400, b=200.5. concat → '400200.5'.
    match r[0] {
        luna_jit::runtime::Value::Str(ref s) => {
            assert_eq!(s.as_bytes(), b"400200.5");
        }
        ref other => panic!("expected Str, got {:?}", other),
    }
}

/// Concat with `__concat` metamethod — helper must detect the
/// frame push (begin_meta_call → push_frame) and deopt. The
/// interp then takes over and the metamethod runs correctly.
/// Uses only Int operand so the trace can actually compile (the
/// metatable-bearing one bails to interp from the helper's
/// frame-push detection path).
#[test]
fn concat_metamethod_deopts_then_interp_succeeds() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local obj = setmetatable({}, {
                 __concat = function(a, b) return 'META' end
             })
             local function f(x)
                 return x .. obj
             end
             local results = ''
             for i = 1, 100 do results = f(i) end
             return results",
        )
        .unwrap();
    match r[0] {
        luna_jit::runtime::Value::Str(ref s) => {
            assert_eq!(s.as_bytes(), b"META");
        }
        ref other => panic!("expected Str, got {:?}", other),
    }
}
