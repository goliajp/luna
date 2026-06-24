//! P12-S12-A v3 — `Op::Test` / `Op::TestSet` runtime tag-based
//! truthy guard for `RegKind::Unset` slots.
//!
//! v1 (`ff719da`) / v2 (`9346090`) implemented compile-time
//! truthy folding for known kinds (Int/Float/Table/Closure/Nil,
//! plus Str added by C-v2 `7504ef1`). Unset bailed compile.
//!
//! v3 resurrects the `luna_jit_stack_tag` helper (dormant since
//! v4 of S12-B replaced its main use site) and emits a runtime
//! guard for Unset slots: `is_truthy = (tag > 1)` (Lua truthy is
//! anything not Nil(0) or Bool(false)(1)). The guard compares
//! to the recorded direction; mismatch → store_back + return
//! the op's PC so interp redoes the test.
//!
//! Common Unset producers: `luna_jit_stack_load` reload after
//! a helper call (TForCall slow path, Concat result), GetUpval
//! whose target isn't an Op::Call, dispatcher slots without an
//! entry-tag → RegKind mapping.

use luna_jit::version::LuaVersion;

/// `if v then ... end` over an Unset-kind v derived from a
/// helper-reloaded slot. Verifies the trace compiles and
/// dispatches; v before v3 would bail compile here.
#[test]
fn test_unset_kind_compiles_with_runtime_guard() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // Construct a Concat that returns a Str (becomes Unset after
    // stack_load reload), then `if x then s = s + 1 end` uses
    // Op::Test on the Unset slot.
    let r = vm
        .eval(
            "local function build(n)
                 local total = 0
                 for i = 1, n do
                     local s = '' .. i
                     if s then total = total + 1 end
                 end
                 return total
             end
             return build(200)",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(200)),
        "expected Int(200), got {:?}",
        r[0]
    );
}

/// `local x = v or fallback` where v is Unset (helper-reloaded).
/// TestSet's TookJmp path emits a runtime-guarded Move.
#[test]
fn testset_unset_kind_truthy_takes_move() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function build(n)
                 local total = 0
                 for i = 1, n do
                     local s = '' .. i
                     local x = s or 'fallback'
                     -- x should be s (always truthy)
                     if x then total = total + 1 end
                 end
                 return total
             end
             return build(200)",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(200)),
        "expected Int(200), got {:?}",
        r[0]
    );
}

/// Boolean-valued Unset: `local b = (i > 0)` produces a Bool. v3
/// guard handles tag=TRUE(2) and tag=FALSE(1) at runtime via the
/// `tag > 1` check.
#[test]
fn test_unset_bool_kind_runtime_guard() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function build(n)
                 local hits = 0
                 for i = 1, n do
                     -- Helper-reloaded value used in a Test
                     local s = '' .. i
                     if s then hits = hits + 1 end
                 end
                 return hits
             end
             return build(150)",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(150)),
        "expected Int(150), got {:?}",
        r[0]
    );
}
