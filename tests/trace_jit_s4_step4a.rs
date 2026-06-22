//! P12-S4-step4a — recorder close-detection unification.
//!
//! Before: call-triggered traces closed on **any** re-entry of
//! head_pc (giving fib a 7-op depth=0 prefix); back-edge traces
//! closed on `cur_depth == 0` re-entry; `MAX_INLINE_DEPTH` cap and
//! `frames.len() <= recording_frame_base` both **aborted** the trace
//! (drop the body, count toward `trace_aborted_count`).
//!
//! After: both trigger flavours use `cur_depth == 0` for the head_pc
//! re-entry close. The depth cap and returned-past-head conditions
//! become **clean closes** — whatever was recorded up to that point
//! is the trace body, eligible to compile (and emit via the existing
//! `TraceEnd::InlineAbort` path until step4b's frame-materialisation
//! helper lands). End result: recorder produces traces with depth>0
//! ops on real Lua code (fib, deep recursion), feeding the inline
//! emit plumbing step3b shipped.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// fib(28) recorder produces a trace containing depth>0 ops (max
/// depth tracker at least 1). Result still correct.
#[test]
fn fib_28_recorder_now_walks_into_callee() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n)
                 if n < 2 then return n end
                 return f(n-1) + f(n-2)
             end
             return f(28)",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(317811)));

    assert!(
        vm.trace_max_depth_seen() >= 1,
        "step4a recorder must walk into the self-recursive callee; \
         got max_depth_seen={}",
        vm.trace_max_depth_seen(),
    );
    assert!(
        vm.trace_closed_count() >= 1,
        "fib's trace must close at least once after step4a; got closed={}",
        vm.trace_closed_count(),
    );
}

/// `frames.len() <= recording_frame_base` no longer aborts — it
/// cleanly closes. Combined with the depth-cap close, the deep-r(6)
/// hot loop should leave `trace_aborted_count == 0` (every recording
/// attempt either compiles or closes via a non-abort path).
#[test]
fn deep_recursion_no_longer_aborts_traces() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function r(n) if n == 0 then return 1 end return 1 + r(n-1) end
             local s = 0
             for i = 1, 200 do s = s + r(6) end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(1400)));

    // Step4a flipped both abort paths (depth-cap, returned-past-head)
    // to clean close. There are no other abort sources for this
    // workload, so aborted_count stays zero.
    assert_eq!(
        vm.trace_aborted_count(),
        0,
        "step4a turned MAX_INLINE_DEPTH + returned-past-head aborts \
         into clean closes; got aborted={}",
        vm.trace_aborted_count(),
    );
    assert!(
        vm.trace_closed_count() >= 1,
        "at least one r trace must close; got closed={}",
        vm.trace_closed_count(),
    );
}

/// Plain back-edge loop closure unchanged — back-edge head_pc
/// re-entry at depth=0 still closes, dispatcher still runs the
/// numeric-for trace.
#[test]
fn numeric_loop_close_path_unchanged() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do s = s + i end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(500500)));
    assert!(
        vm.trace_dispatched_count() >= 1,
        "numeric for trace must still dispatch after step4a; got dispatched={}",
        vm.trace_dispatched_count(),
    );
}
