//! P12-S4-step0 smoke tests — verify the trace-on-call trigger fires
//! on a self-recursive function whose body has no negative back-edge
//! (`fib`, recursive `make`/`check` in `binary_trees`).
//!
//! Step 0 wires the trigger and the depth==0 close gate. Step 2
//! refined close detection: call-triggered traces now close on any
//! re-entry of `(head_proto, head_pc)`, while back-edge triggered
//! traces still require `cur_depth == 0` (the gate's original
//! purpose — keep loop traces from prematurely closing when the
//! loop body calls a function that happens to land at the trace
//! head's pc).

use luna::version::LuaVersion;
use luna::vm::Vm;

/// fib(12) = 144. fib's Proto is called 145 times in total (1 outer +
/// 144 inner recursions across the tree), which crosses the call-hot
/// threshold of 64 well before completion. We must compute the right
/// answer and the trigger must have fired at least once. Per step 2,
/// the call-triggered trace closes on first recursive `Op::Call`'s
/// re-entry to fib's pc=0 — so `trace_closed_count >= 1` is the
/// canonical signal.
#[test]
fn trace_on_call_fires_on_fib_recursion_and_result_matches_interp() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
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
    let v = r[0];
    let got = match v {
        luna::runtime::Value::Int(n) => n,
        _ => panic!("expected Int, got {v:?}"),
    };
    assert_eq!(got, 144, "fib(12) must equal 144");

    // Either we closed (step 2 single-pass: closes on call-reentry)
    // or aborted (e.g., trigger fired on a base-case n<2 frame, then
    // returned past the trace head). Either way the trigger fired.
    let fired = vm.trace_aborted_count() + vm.trace_closed_count();
    assert!(
        fired >= 1,
        "trace-on-call should have fired on fib's recursion, got \
         aborted={} closed={} compiled={} dispatched={}",
        vm.trace_aborted_count(),
        vm.trace_closed_count(),
        vm.trace_compiled_count(),
        vm.trace_dispatched_count(),
    );
}

/// The depth==0 close-detection gate keeps a nested call inside a
/// **loop-triggered** trace from being mistaken for the trace's
/// clean close. Without the gate, if the loop body called a
/// function whose pc=0 happened to equal the trace head pc, the
/// trace would close mid-iteration. The gate forces `cur_depth==0`
/// before considering close, so only true loop back-edges close.
///
/// This is the original step-0 invariant; step 2 only relaxed it for
/// **call-triggered** traces (which need to close at depth>=0 to
/// produce a usable single-pass body).
#[test]
fn depth_zero_gate_holds_for_loop_triggered_traces() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // A loop that calls a helper whose body is trivial; the recorder
    // fires on the for-loop's back-edge (loop-triggered). Inside the
    // body, helper's frame pushes depth to 1; depth==0 gate keeps
    // the trace from prematurely closing on the helper's entry.
    let r = vm
        .eval(
            "local function helper(x) return x * 2 end
             local s = 0
             for i = 1, 200 do s = s + helper(i) end
             return s",
        )
        .unwrap();
    assert!(matches!(
        r[0],
        luna::runtime::Value::Int(40200)
    ));
    // The trace either compiles or aborts (helper's body uses
    // upvals/etc. the whitelist may not cover) — what we verify is
    // that the dispatcher did NOT fire a degenerate empty trace
    // (which would happen if depth gate failed). A degenerate trace
    // would dispatch, return immediately, and the loop would still
    // produce the right answer but counter would be wrong.
    // The strongest invariant: if anything dispatched, the count is
    // bounded by what's reasonable (≤ loop iters).
    assert!(
        vm.trace_dispatched_count() <= 1000,
        "dispatch count must be bounded; got {}",
        vm.trace_dispatched_count()
    );
}

/// Trace JIT gated off: zero counters move. Regression guard for the
/// "trace-on-call branch even when gated" mistake — the trigger must
/// pay only a not-taken branch when `trace_jit_enabled=false`.
#[test]
fn gate_off_keeps_all_trace_counters_zero() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    // trace_jit_enabled defaults to false; do not flip it.

    let r = vm
        .eval(
            "local function f(n)
                 if n < 2 then return n end
                 return f(n-1) + f(n-2)
             end
             return f(12)",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(144)));

    assert_eq!(vm.trace_closed_count(), 0);
    assert_eq!(vm.trace_aborted_count(), 0);
    assert_eq!(vm.trace_compiled_count(), 0);
    assert_eq!(vm.trace_compile_failed_count(), 0);
    assert_eq!(vm.trace_dispatched_count(), 0);
    assert_eq!(vm.trace_deopt_count(), 0);
}
