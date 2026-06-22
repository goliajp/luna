//! P12-S12-D — hardening tests for the S12 sub-step family.
//!
//! All previous S12 tests cover the **success path** of each
//! feature (whitelist + emit + dispatch). v12-D fills the
//! **sad/edge path** corners shipped during S12-A/B/C but not
//! explicitly exercised:
//!
//! - empty-iter generic for (ipairs over `{}`) — recorder must
//!   not trigger, no compile failure
//! - deep Move shuffle into Concat — verifies operand spill works
//!   even when the operand chain is several Move levels removed
//!   from the original Str entry slot
//! - entry-tag cross-call shift — a function called with a Str
//!   on one call then an Int on the next; the S12-C v3 dispatcher
//!   entry guard must reject the second call's dispatch and let
//!   interp handle it without panic / corruption
//! - ipairs trace that runs after a deopt — verifies the deopt
//!   restore path properly cleans up so subsequent dispatches
//!   continue working
//!
//! These tests don't ship new emit; they exercise paths already
//! shipped but not previously asserted on.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// Empty-iter ipairs: TForLoop's `take_back_edge` gate (`ctrl !=
/// nil`) is false on the very first call to the iter — so the
/// recorder trigger must NOT fire. Verify aborted_count stays 0
/// and the loop produces the expected (zero) sum.
#[test]
fn empty_ipairs_does_not_trigger_recorder() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for outer = 1, 500 do
                 for _, v in ipairs({}) do
                     s = s + v
                 end
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(0)),
        "expected Int(0), got {:?}",
        r[0]
    );
    assert_eq!(
        vm.trace_aborted_count(),
        0,
        "empty-iter generic for must not abort recorder; \
         aborted={}",
        vm.trace_aborted_count(),
    );
}

/// Deep Move shuffle into Concat: `s = (a) .. (b)` where a, b
/// flow through multiple temp slots. Verifies the v2 Str-kind
/// propagation across Move chains lands the right tag at the
/// Concat operand spill.
#[test]
fn deep_move_chain_into_concat() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function build(n)
                 local last = ''
                 for i = 1, n do
                     local a = 'lhs-'
                     local b = 'rhs'
                     local x = a
                     local y = b
                     last = x .. y
                 end
                 return last
             end
             return build(200)",
        )
        .unwrap();
    match r[0] {
        luna::runtime::Value::Str(ref s) => {
            assert_eq!(s.as_bytes(), b"lhs-rhs");
        }
        ref other => panic!("expected Str, got {:?}", other),
    }
}

/// Entry-tag cross-call shift: a function `f(x)` called with an
/// Int x to hot-trigger a trace specialised to entry_tag[arg]=Int,
/// then called with a Str x. The S12-C v3 dispatcher entry guard
/// must skip dispatch on the Str call and let interp handle it
/// without panic. Result correctness is the assertion.
#[test]
fn entry_tag_cross_call_shift_no_panic() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(x)
                 local s = 0
                 for i = 1, 100 do
                     if x then s = s + 1 end
                 end
                 return s
             end
             -- Hot-trigger with Int x (truthy):
             local int_sum = 0
             for k = 1, 30 do int_sum = f(k) end
             -- Now call with a Str: entry guard must not crash.
             local str_sum = f('hello')
             -- And with nil (falsy):
             local nil_sum = f(nil)
             return int_sum + str_sum + nil_sum",
        )
        .unwrap();
    // int_sum = 100 (truthy each iter), str_sum = 100 (Str truthy),
    // nil_sum = 0 (nil falsy). Total 200.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(200)),
        "expected Int(200), got {:?}",
        r[0]
    );
}

/// ipairs trace + Op::Test runtime guard exercise: Test inside the
/// inner loop body where the test slot can be Unset post-Concat-
/// reload. Verifies that the post-deopt restore + reentry to interp
/// + subsequent dispatches all work cleanly.
#[test]
fn ipairs_with_test_unset_kind_inner_body() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {'a', 'b', 'c'}
             local function build(n)
                 local hits = 0
                 for _ = 1, n do
                     for _, v in ipairs(t) do
                         if v then hits = hits + 1 end
                     end
                 end
                 return hits
             end
             return build(50)",
        )
        .unwrap();
    // 50 outer iters × 3 truthy v per inner iter = 150.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(150)),
        "expected Int(150), got {:?}",
        r[0]
    );
}
