//! P12-S12-B v2 — `Op::TForCall` + `Op::TForLoop` trace JIT emit.
//!
//! v1 (`c1aa94a`) wired the recorder back-edge trigger for generic-for.
//! v2 adds the rest: whitelist `TForPrep` / `TForCall` / `TForLoop`,
//! the `luna_jit_op_tforcall` helper (Native iters only — Lua-closure
//! iters deopt), and the TForLoop tail emit (stack_tag → exit on Nil,
//! continue on Int, deopt on other).
//!
//! These tests assert end-to-end correctness AND that the dispatch
//! path is now exercised (`trace_dispatched_count > 0` after enough
//! iterations).

use luna::version::LuaVersion;
use luna::vm::Vm;

/// Canonical ipairs-over-Int-array — the v2 sweet spot. After the
/// 64-iter warmup, the inner ipairs loop dispatches per outer iter
/// (each dispatch runs 8 inner iters until ipairs returns nil).
#[test]
fn tforcall_ipairs_int_array_dispatches() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
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
    assert!(
        matches!(r[0], luna::runtime::Value::Int(7200)),
        "expected Int(7200), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_compiled_count() >= 1,
        "expected at least one trace to compile; compiled_count={}",
        vm.trace_compiled_count(),
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "expected generic-for trace to actually dispatch in v2; \
         dispatched_count={}",
        vm.trace_dispatched_count(),
    );
}

/// Empty-iter case: `pairs({})` calls iter once which returns Nil,
/// so the TForLoop take_back_edge gate is false on the very first
/// hit and the recorder never starts. v2 must keep this guarantee
/// (don't pollute hot_count, don't spin the recorder).
#[test]
fn tforcall_empty_iter_does_not_dispatch() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
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
        matches!(r[0], luna::runtime::Value::Int(0)),
        "expected Int(0), got {:?}",
        r[0]
    );
    // Aborted should stay 0 — empty inner iter never enters the
    // recorder (TForLoop's take_back_edge gate is false).
    assert_eq!(
        vm.trace_aborted_count(),
        0,
        "empty-iter generic for must not abort the recorder; \
         aborted_count={}",
        vm.trace_aborted_count(),
    );
}

/// Variable-size Int arrays — exercise the Nil side-exit at the
/// natural loop end. Each outer iter has a different table length,
/// so TForLoop's Nil exit fires at different per-iter positions.
#[test]
fn tforcall_ipairs_varying_lengths() {
    let mut vm = luna::new_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for outer = 1, 100 do
                 local t = {}
                 for j = 1, outer do t[j] = j end
                 for _, v in ipairs(t) do
                     s = s + v
                 end
             end
             return s",
        )
        .unwrap();
    // For outer=N, inner sum = N*(N+1)/2. Total = sum over N=1..100 =
    // sum(N*(N+1)/2) = (1/2) * sum(N^2 + N) = (1/2) * (338350+5050) = 171700.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(171700)),
        "expected Int(171700), got {:?}",
        r[0]
    );
}
