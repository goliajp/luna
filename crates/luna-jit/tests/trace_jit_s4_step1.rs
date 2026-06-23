//! P12-S4-step1 — recorder fills `RecordedOp.inline_depth` with the
//! live nesting depth relative to the trace head's frame, and bails
//! the trace when the depth crosses `MAX_INLINE_DEPTH`.
//!
//! Step 1 only changes the recorder. The lowerer still bails on
//! `inline_depth > 0`, so no compile / dispatch from depth>0 ops yet
//! — but `trace_max_depth_seen` lets tests verify the tracker really
//! walks past 0 on recursive / nested-call bodies.
//!
//! Step 2 changed call-triggered traces to close on first re-entry,
//! so the depth tracker for fib-style recursion is rarely exercised
//! through that path now. These tests therefore use **back-edge
//! triggered** traces (a hot for-loop) whose body **calls** into
//! another function — that's the path that genuinely pushes depth
//! past 0 today.

use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// A hot for-loop whose body calls a helper function. The recorder
/// fires on the for-loop's back-edge; inside the loop body, the
/// `Op::Call` into `helper` bumps depth to 1 and the helper's body
/// is recorded at depth=1. The depth tracker must observe at least
/// one op at `inline_depth >= 1`.
#[test]
fn recorder_observes_depth_above_zero_on_nested_call() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function helper(n) return n + 1 end
             local s = 0
             for i = 1, 1000 do s = s + helper(i) end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(501500)));

    assert!(
        vm.trace_max_depth_seen() >= 1,
        "recorder must have observed depth >= 1 on the nested helper \
         call; got max_depth_seen={} aborted={} closed={}",
        vm.trace_max_depth_seen(),
        vm.trace_aborted_count(),
        vm.trace_closed_count(),
    );
}

/// A plain `for` loop with no nested call should keep depth at 0
/// throughout — the depth tracker must not false-positive on
/// straight-line bytecode.
#[test]
fn recorder_keeps_depth_zero_on_pure_loop() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do s = s + i end
             return s",
        )
        .unwrap();
    assert!(matches!(
        r[0],
        luna_jit::runtime::Value::Int(500500)
    ));
    assert_eq!(
        vm.trace_max_depth_seen(),
        0,
        "straight-line loop must keep depth at 0; got {}",
        vm.trace_max_depth_seen()
    );
}

/// Deep nested-call recursion in a hot loop hits the
/// `MAX_INLINE_DEPTH` cap. P12-S4-step4a flipped the cap action from
/// `abort` to `clean close` (the trace's body up to the cap forms the
/// recorded body); count moved from `trace_aborted_count` to
/// `trace_closed_count`. depth observation still saturates at
/// MAX_INLINE_DEPTH (the over-cap op never pushes).
///
/// P13-S13-B/C (`MAX_INLINE_DEPTH = 4 → 8 → 16`): widen the chain
/// further so the recursion always over-runs the latest cap. The
/// saturation assertion tracks `MAX_INLINE_DEPTH` directly.
#[test]
fn deep_nested_calls_in_hot_loop_trigger_max_inline_depth_close() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // r(20) → 20-level recursion; definitely exceeds MAX_INLINE_DEPTH=16.
    let r = vm
        .eval(
            "local function r(n) if n == 0 then return 1 end return 1 + r(n-1) end
             local s = 0
             for i = 1, 200 do s = s + r(20) end
             return s",
        )
        .unwrap();
    // r(20) returns 21; 200 * 21 = 4200.
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(4200)));

    assert!(
        vm.trace_closed_count() >= 1,
        "deep nested recursion should have triggered at least one \
         trace close (depth-cap / returned-past-head); \
         got aborted={} closed={}",
        vm.trace_aborted_count(),
        vm.trace_closed_count()
    );
    let max_depth_cap = luna_jit::jit::trace::MAX_INLINE_DEPTH;
    assert!(
        vm.trace_max_depth_seen() <= max_depth_cap,
        "depth observation must saturate at MAX_INLINE_DEPTH = {}; \
         got {}",
        max_depth_cap,
        vm.trace_max_depth_seen()
    );
}
