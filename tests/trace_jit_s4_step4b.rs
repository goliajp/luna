//! P12-S4-step4b-C-2 — inline cmp@d>0 side-exits dispatch via the
//! frame-materialisation helper. Each cmp@d>0 site builds its OWN
//! `FrameMaterializeInfo` chain (sibling self-rec Calls in fib's
//! body have DIFFERENT chains under each branch — the v1 attempt
//! that used a single global array indexed by depth looped fib
//! forever).
//!
//! The headline test runs fib(28) under the trace JIT and asserts
//! (1) the correct result and (2) that the trace dispatches, with a
//! finite count proving the C-2 redo broke the v1 infinite loop.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// fib(28) under inline self-rec dispatch — the v2 of step4b-C-2's
/// per-cmp-site array. Asserts the recursive descent computes
/// 317811 and the dispatch counter is non-zero and FINITE (the v1
/// failure mode was an infinite loop, so a successful return is
/// itself the no-loop guarantee).
#[test]
fn fib_28_dispatches_via_inline_chain() {
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
        vm.trace_compiled_count() >= 1,
        "fib's trace must compile under step4b-C-2; got compiled={}",
        vm.trace_compiled_count()
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "step4b-C-2 lifts the length-gate for inline traces — fib should dispatch; \
         got dispatched={}",
        vm.trace_dispatched_count()
    );
    // No deopt expected for fib's straight-line numeric body.
    assert_eq!(
        vm.trace_deopt_count(),
        0,
        "fib has no helper that can park a deopt; expected deopt=0, got {}",
        vm.trace_deopt_count()
    );
}

/// fib(20) — same shape, smaller value — sanity check that the
/// dispatch + side-exit + Return cycle survives many side-exits in
/// a row without state drift.
#[test]
fn fib_20_correct_under_inline_dispatch() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n)
                 if n < 2 then return n end
                 return f(n-1) + f(n-2)
             end
             return f(20)",
        )
        .unwrap();
    assert!(matches!(r[0], luna::runtime::Value::Int(6765)));
}

/// Numeric-for loops (no inline self-rec) still use the legacy
/// non-inline dispatch path. Regression guard against the cmp emit
/// refactor breaking the cont_pc-based per_exit_tags lookup.
#[test]
fn numeric_loop_still_dispatches_after_c2() {
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
        "non-inline numeric-for trace must still dispatch; got dispatched={}",
        vm.trace_dispatched_count()
    );
}
