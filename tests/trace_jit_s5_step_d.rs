//! P12-S5-D — sunk emit extends to looping (ForLoop-terminated)
//! traces. The escape sweep no longer auto-escapes live bindings
//! at a `TraceEnd::ForLoop` terminator: interp resumes OUTSIDE
//! the loop on exit, where any `local t = {...}` declared inside
//! the body is out of scope (parser frees the register slot).
//! The exit-tag override (Sinkable slot → Untouched) keeps the
//! restored slot reading as its entry tag.
//!
//! These tests verify:
//! 1. Positive — a `for ... do local t = {a,b,c}; ... end` loop
//!    body's NewTable sinks; the trace dispatches ONCE (native
//!    back-edge until ForLoop exits) and skips every heap alloc.
//! 2. Negative — a body cmp (`if x > 0 then ... end` inside the
//!    loop) blocks sunk emit via the `body_has_cmp` pre-emit
//!    gate; result stays correct via the heap helper path.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// `for i = 1, 1000 do local t = {1,2,3}; s = s + t[1]+t[2]+t[3] end`.
/// Each iter's NewTable is sunk; the trace dispatches once (the
/// internal-loop back-edge runs the body 1000 times natively until
/// ForLoop's side-exit fires). 1000 * (1+2+3) = 6000.
#[test]
fn for_body_sunk_dispatches_once() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {1, 2, 3}
                 s = s + t[1] + t[2] + t[3]
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(6000)),
        "expected Int(6000), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "for-body NewTable must take sunk-emit; got sunk_alloc_count={}, \
         sinkable_seen={}, compiled={}",
        vm.trace_sunk_alloc_count(),
        vm.trace_sinkable_seen_count(),
        vm.trace_compiled_count()
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "looping sunk trace must dispatch; got dispatched={}",
        vm.trace_dispatched_count()
    );
    // The internal-loop tail is what makes the dispatch count tight:
    // ideally exactly 1 entry runs all 1000 iters natively. Allow a
    // small slack for the recorder's pre-trigger iters that ran in
    // interp before the back-edge counter crossed the threshold.
    assert!(
        vm.trace_dispatched_count() < 10,
        "internal-loop tail means dispatch fires ~once, not per-iter; \
         got dispatched={}",
        vm.trace_dispatched_count()
    );
    assert_eq!(
        vm.trace_deopt_count(),
        0,
        "no helper that can park a deopt fires on the sunk path; got deopt={}",
        vm.trace_deopt_count()
    );
}

/// `for i=1,1000 do local t={1,2,3}; if i > 500 then s = s + t[1] end end`.
/// The body has an `Op::Lt` (the `i > 500` test). Post-S5-C, the
/// site STILL sinks: the cmp side-exit's emit path materialises
/// the live virt slots into a heap `Gc<Table>` so the interp
/// resume sees the correct table at `t`. Result asserts
/// correctness via the materialise-on-deopt path.
#[test]
fn for_body_with_cmp_now_sinks_and_materialises() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {1, 2, 3}
                 if i > 500 then s = s + t[1] end
             end
             return s",
        )
        .unwrap();
    // i = 501..1000 → 500 iters, each adds t[1]=1 → s = 500.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(500)),
        "expected Int(500), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "post-S5-C, body cmp does not block sunk emit; \
         got sunk_alloc_count={}",
        vm.trace_sunk_alloc_count()
    );
    assert!(
        vm.trace_materialize_emit_count() >= 1,
        "the cmp side-exit must emit a materialise call for the \
         live sunk site; got materialize_emit_count={}",
        vm.trace_materialize_emit_count()
    );
}
