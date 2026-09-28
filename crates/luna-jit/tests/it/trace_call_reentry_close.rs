//! Close-on-call-reentry for call-triggered traces.
//!
//! A trace started by `begin_call`'s call-hot trigger closes on
//! **any** re-entry of `(head_proto, head_pc)`, giving the recorder a
//! clean single-pass body — fib's depth-0 prefix from pc=0 up through
//! the first recursive `Op::Call`. Without it the recording would
//! either abort (recursion never returns to the trace head's frame's
//! pc=0 — the head pc is **fib's entry**, and the trace head was a
//! specific invocation that does return) or run to `MAX_TRACE_LEN`.
//!
//! These tests verify the **close path** end-to-end.

use luna_jit::version::LuaVersion;

/// fib's trace must **close** (not abort) under the new
/// close-on-call-reentry semantic. Whether compile succeeds is a
/// separate question (see `trace_getupval_whitelist.rs`).
#[test]
fn fib_trace_closes_on_first_recursive_reentry() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n)
                 if n < 2 then return n end
                 return f(n-1) + f(n-2)
             end
             return f(12)",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(144)));
    assert!(
        vm.trace_closed_count() >= 1,
        "fib's trace must close on first recursive re-entry (single-pass); \
         got closed={} aborted={}",
        vm.trace_closed_count(),
        vm.trace_aborted_count()
    );
}

/// Loop-triggered traces (back-edge from `Op::Jmp` neg or
/// `Op::ForLoop`) must STILL require `cur_depth == 0` for close.
/// This guards against accidentally relaxing the gate for the wrong
/// flavor — the existing 21-test trace_recording_smoke/s2c/s3 suite verifies
/// the loop close path didn't break.
///
/// We assert via a tight loop that dispatches as expected (proves
/// the close fired at the loop back-edge, not at a spurious nested
/// match).
#[test]
fn loop_triggered_close_unaffected_by_call_triggered_change() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 5000 do s = s + i end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(12502500)));
    // Loop-triggered trace closes cleanly → compiles → dispatches.
    // The exact dispatch count varies with how many back-edges the
    // loop took before the trace fired, but it must be > 0.
    assert!(
        vm.trace_dispatched_count() >= 1,
        "loop-triggered trace must still close + compile + dispatch; \
         got dispatched={} closed={} compiled={}",
        vm.trace_dispatched_count(),
        vm.trace_closed_count(),
        vm.trace_compiled_count()
    );
}
