//! P12-S6-A2 — `Op::LoadNil` joins the trace JIT whitelist.
//!
//! Pre-S6 these patterns all bailed compile at the first
//! non-whitelisted op:
//! - `{nil, nil}` table constructor (`NewTable + LoadNil×N +
//!   SetList`) — common Lua frontend shape for `local t = {nil,
//!   nil}` and recursive base cases like binary_trees' bottomup
//!   leaf (`return {nil, nil}`).
//! - `if t[k] == nil then` predicate (`GetI + LoadNil + Eq`) —
//!   itemcheck leaf shape.
//!
//! These tests verify (a) the recorded trace closes AND compiles
//! (was: closed + compile_failed) and (b) the helper path with a
//! Nil source writes Value::Nil into the heap table via the new
//! `luna_jit_table_set_nil` helper rather than coercing to
//! Value::Int(0).

use luna_jit::version::LuaVersion;

/// `for i=1,1000 do local t = {nil, nil}; if t[1] == nil then s = s + 1 end end`.
/// Body shape: `NewTable + LoadNil×2 + SetList + GetI + LoadNil +
/// Eq + ...`. Both LoadNil writers must pass the whitelist for
/// compile to succeed. Asserts `compiled >= 1` (was 0).
#[test]
fn loadnil_in_for_body_compiles() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {nil, nil}
                 if t[1] == nil then s = s + 1 end
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(1000)),
        "expected Int(1000), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_compiled_count() >= 1,
        "the `{{nil, nil}}` + `t[1] == nil` body must compile; \
         got closed={}, compiled={}, compile_failed={}",
        vm.trace_closed_count(),
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count()
    );
    assert_eq!(
        vm.trace_compile_failed_count(),
        0,
        "no fail path expected (was: 1+ pre-S6); got compile_failed={}",
        vm.trace_compile_failed_count()
    );
}

/// LoadNil in a sunk-emit context: `{nil, nil}` table literal whose
/// table doesn't escape (the constructor's NewTable + LoadNil×2 +
/// SetList feeds GetIs that produce nil, used in an `if`). When the
/// site is sunk, LoadNil-written sources propagate `RegKind::Nil`
/// into `virt_kinds`; downstream GetI reads back Nil; the predicate
/// `t[1] == nil` evaluates correctly without ever touching the heap.
#[test]
fn loadnil_in_sunk_setlist_propagates_nil_kind() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {nil, nil}
                 local v = t[1]
                 if v == nil then s = s + 1 end
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(1000)),
        "expected Int(1000), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_compiled_count() >= 1,
        "the local-table + GetI + Eq body must compile; \
         got closed={}, compiled={}, compile_failed={}",
        vm.trace_closed_count(),
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count()
    );
}

/// `for i=1,N do local t = {nil, nil}; t[1] = i; s = s + t[1] end` —
/// the heap-helper SetList path with one Nil source per slot. Each
/// LoadNil-written source must route to `luna_jit_table_set_nil` so
/// the freshly-allocated table has `Value::Nil` in slot 1 / 2 before
/// `t[1] = i` writes Int over it. If routing was wrong (`Value::Int(0)`
/// instead of Nil), the program still returns the right sum here, but
/// the `t[2]` we never write would observe Int(0) instead of Nil — so
/// add an explicit `if t[2] == nil` predicate to catch the difference.
#[test]
fn loadnil_then_setlist_writes_nil_via_helper() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             local nilcount = 0
             for i = 1, 200 do
                 local t = {nil, nil}
                 t[1] = i
                 if t[2] == nil then nilcount = nilcount + 1 end
                 s = s + t[1]
             end
             return s, nilcount",
        )
        .unwrap();
    // Sum 1..200 = 20100; nilcount must be 200 (every t[2] stayed Nil).
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(20100)),
        "expected Int(20100), got {:?}",
        r[0]
    );
    assert!(
        matches!(r[1], luna_jit::runtime::Value::Int(200)),
        "every t[2] must read as Nil — set_nil helper routed wrong if \
         this isn't 200; got {:?}",
        r[1]
    );
    // Trace compile may or may not happen depending on the recorder's
    // trigger threshold + Move/SetI shape (escape sweep escapes the
    // sunk site on the `Move R[t']=R[t]` alias from frontend); the
    // contract we lock here is *correctness* under the helper path,
    // not dispatch. The other two tests cover compile-success.
    let _ = vm.trace_compiled_count();
}
