//! P12-S12-B v1 — `Op::TForLoop` trace JIT back-edge trigger.
//!
//! Pre-S12-B, generic-for loops (`for k,v in expr do ... end`) NEVER
//! entered the trace recorder. The recorder is plumbed through
//! `Op::Jmp` neg-offset (`while`/`repeat`) and `Op::ForLoop` (numeric
//! `for`); `Op::TForLoop` was the missing third back-edge. v1 wired
//! the trigger; v2 (now landed) added whitelist + helper + emit so
//! generic-for traces actually compile + dispatch. Tests here verify
//! the recorder still triggers and correctness holds end-to-end. The
//! v2 dispatch-counter assertions live in `trace_jit_s12_step_b_v2.rs`.

use luna_jit::version::LuaVersion;

/// `ipairs` over a non-empty array, looped well past TRACE_HOT_THRESHOLD
/// (=64). Verifies recorder triggers AND end-to-end correctness.
#[test]
fn tforloop_back_edge_triggers_recorder_close() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local t = {}
             for i = 1, 8 do t[i] = i end
             local s = 0
             for iter = 1, 200 do
                 for _, v in ipairs(t) do
                     s = s + v
                 end
             end
             return s",
        )
        .unwrap();
    // 200 outer iters * (1+2+...+8) = 200 * 36 = 7200.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(7200)),
        "expected Int(7200), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_closed_count() >= 1,
        "expected trace recorder to close on TForLoop back-edge; \
         closed_count={}",
        vm.trace_closed_count(),
    );
}

/// `pairs` over an empty table — `next(t, nil)` returns nil on the
/// very first call, so `Op::TForLoop`'s `ctrl != nil` gate is false
/// and the back-edge never fires. Trigger must not bump hot_count
/// → recorder must NOT start a trace. Verifies the take_back_edge
/// gating mirrors Op::ForLoop's `count > 0` semantics.
#[test]
fn tforloop_empty_iter_does_not_trigger() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for iter = 1, 500 do
                 for _, v in pairs({}) do
                     s = s + v
                 end
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(0)),
        "expected Int(0), got {:?}",
        r[0]
    );
    // 500 iters of pairs over {} → next(t,nil) returns nil → TForLoop
    // ctrl is nil → take_back_edge=false → trigger gate fails. Empty
    // generic for must NOT pollute hot_count nor start a recording.
    // (The OUTER numeric-for runs 500 iters, which does compile a
    // trace — so trace_closed_count CAN be non-zero. We only care
    // that the recorder didn't ABORT due to a generic-for shape it
    // can't handle yet.)
    assert_eq!(
        vm.trace_aborted_count(),
        0,
        "empty-iter generic for must not pollute the recorder; \
         aborted_count={}",
        vm.trace_aborted_count(),
    );
}
