//! P12-S7-A — `Op::Closure` joins the trace JIT whitelist (shared
//! upvals + 0 upvals only; `in_stack: true` upvals bail compile
//! and ship in S7-B).
//!
//! Pre-S7 every trace containing an `Op::Closure` bailed compile
//! at the first non-whitelisted op. Three patterns this RFC opens:
//! - `local function f() return function() return 1 end end` —
//!   inner has 0 upvals, f's body trace = `Closure + Return1`.
//! - `function() return outer end` from a hot loop where `outer`
//!   was already an upvalue (shared, `in_stack=false`).
//! - Negative: in_stack upval (`local x = i; return function()
//!   return x end`) still bails until S7-B.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// `local function f() return function() return 1 end end` — the
/// inner closure has 0 upvals. Trace fires on f's body (Closure +
/// Return1, length 2 but length-gate skipped since `closure_seen > 0`).
#[test]
fn closure_no_upval_compiles() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f() return function() return 1 end end
             local s = 0
             for i = 1, 1000 do
                 local g = f()
                 s = s + g()
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(1000)),
        "expected Int(1000), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_compiled_count() >= 1,
        "f body (Closure + Return1) must compile post-S7-A; \
         got closed={}, compiled={}, failed={}",
        vm.trace_closed_count(),
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count()
    );
    assert!(
        vm.trace_closure_emit_count() >= 1,
        "the Op::Closure helper emit must fire ≥1 time; got {}",
        vm.trace_closure_emit_count()
    );
}

/// Closure-returning function nested two levels deep: the middle
/// proto creates a closure that captures the OUTER chunk's `_ENV`
/// upval (shared, `in_stack=false`). Verifies the shared-upval
/// helper path with a non-empty upvals slice.
#[test]
fn closure_with_shared_upval_compiles() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // `f` itself has `_ENV` as upval (shared from outer chunk).
    // `f` creates inner `function() return math.huge end` which has
    // `_ENV` as upval (shared from f, in_stack=false).
    let r = vm
        .eval(
            "local function f()
                 return function() return math.huge end
             end
             local s = 0
             for i = 1, 1000 do
                 local g = f()
                 if g() > 0 then s = s + 1 end
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(1000)),
        "expected Int(1000), got {:?}",
        r[0]
    );
    // f body trace should compile (Closure + Return1, shared _ENV).
    // The downstream g() Call truncates trace at outer.
    assert!(
        vm.trace_compiled_count() >= 1,
        "shared-upval Op::Closure must compile; \
         got closed={}, compiled={}, failed={}",
        vm.trace_closed_count(),
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count()
    );
    assert!(
        vm.trace_closure_emit_count() >= 1,
        "Op::Closure helper emit must fire; got {}",
        vm.trace_closure_emit_count()
    );
}

/// `local function f(x) return function() return x end end` — the
/// inner closure has `x` as an `in_stack: true` upval (captured
/// from f's R[0]). S7-B plumbs the per-upval pre-Closure spill,
/// so f's body trace now compiles AND each iter's closure correctly
/// captures the iter-specific x via an open upval pointing to a
/// freshly-spilled stack slot.
#[test]
fn closure_with_in_stack_upval_compiles_via_spill() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(x) return function() return x end end
             local s = 0
             for i = 1, 1000 do
                 local g = f(i)
                 s = s + g()
             end
             return s",
        )
        .unwrap();
    // Sum 1..1000 = 500500. Each g() reads its iter's x via the
    // open upval — if spill was wrong (e.g. constant tag, stale
    // payload), this sum diverges.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(500500)),
        "expected Int(500500), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_closure_emit_count() >= 1,
        "in_stack Op::Closure must lower via S7-B spill; got \
         closure_emit_count={}",
        vm.trace_closure_emit_count()
    );
}

/// Two in_stack upvals (`x, y`) — each iter's spill writes both
/// caller slots before op_closure helper builds open upvals.
/// Verifies the per-upval spill loop emits all spills, not just
/// the first.
#[test]
fn closure_with_two_in_stack_upvals_compiles() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(x, y) return function() return x + y end end
             local s = 0
             for i = 1, 1000 do
                 local g = f(i, i * 2)
                 s = s + g()
             end
             return s",
        )
        .unwrap();
    // x + y = i + 2i = 3i; sum 3*(1+...+1000) = 3 * 500500 = 1501500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(1501500)),
        "expected Int(1501500), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_closure_emit_count() >= 1,
        "two-in_stack-upval Op::Closure must compile; got \
         closure_emit_count={}",
        vm.trace_closure_emit_count()
    );
}

/// Mixed: closure with one in_stack upval (`x` captured from local)
/// AND one shared upval (`_ENV` carried from outer chunk). Spill
/// must fire for the in_stack one only; shared comes from
/// `cl.upvals()` unchanged.
#[test]
fn closure_with_mixed_shared_and_in_stack_compiles() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(x) return function() return x + math.huge end end
             local s = 0
             for i = 1, 1000 do
                 local g = f(i)
                 if g() > i then s = s + 1 end
             end
             return s",
        )
        .unwrap();
    // x + math.huge > x always (math.huge is inf), so s = 1000.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(1000)),
        "expected Int(1000), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_closure_emit_count() >= 1,
        "mixed-upval Op::Closure must compile; got \
         closure_emit_count={}",
        vm.trace_closure_emit_count()
    );
}
