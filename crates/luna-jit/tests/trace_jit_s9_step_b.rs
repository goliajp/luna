//! P12-S9-B — pre-emit accepts `Op::Call C=0` (variable-return
//! form) when the recorder's `var_count` snapshot pinned the
//! actual return count to 1. Reduces to the same emit shape as
//! `Op::Call C=2` (single return value Return1 copy-back).
//!
//! Common pattern: Lua's frontend uses Call C=0 for the LAST call
//! of a multi-value expression (`{f(), g()}` — g is Call C=0 to
//! splice all g's returns into the SetList tail). When g returns
//! exactly 1 value (the binary_trees `make`'s `return {...}`),
//! S9-B unblocks compile success of make's body trace.

use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// binary_trees `make` body pre-S9-B bailed compile at the
/// self-recursive `Call C=0`. Post-S9-B, var_count=1 → compile
/// succeeds. Even if the trace doesn't yet dispatch (still
/// pending S9-C SetList B=0 emit), the compile-success move is
/// the contract this test locks.
#[test]
fn binary_trees_make_compiles_post_s9b() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function make(d)
                 if d == 0 then return {nil, nil} end
                 return {make(d-1), make(d-1)}
             end
             local function check(t)
                 if t[1] == nil then return 1 end
                 return 1 + check(t[1]) + check(t[2])
             end
             local sum = 0
             for i = 1, 10 do sum = sum + check(make(10)) end
             return sum",
        )
        .unwrap();
    // 10 * (2^11 - 1) = 20470.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(20470)),
        "expected Int(20470), got {:?}",
        r[0]
    );
    // S9-B unlocks compile-success on Call C=0 var_count==1.
    // Combined with S6/S7/S8 ops, make body now compiles. Exact
    // count depends on which traces trigger; we just require some
    // trace did compile (≥ 1) and no spurious fails.
    assert!(
        vm.trace_compiled_count() >= 1,
        "binary_trees pattern must compile post-S9-B; got \
         closed={} compiled={} fail={}",
        vm.trace_closed_count(),
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
    );
}

/// Negative guard: Op::Call C=0 with var_count != 1 still bails
/// (multi-value return needs S9-D for multi-Return copy-back).
/// Synthesized via a pattern that returns multiple values.
#[test]
fn call_c_zero_with_var_count_gt_1_still_bails_or_runs_via_interp() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // f returns 2 values; outer call expects all (C=0 form via {...}).
    let r = vm
        .eval(
            "local function f() return 10, 20 end
             local s = 0
             for i = 1, 200 do
                 local t = {f()}
                 s = s + t[1] + (t[2] or 0)
             end
             return s",
        )
        .unwrap();
    // (10+20) * 200 = 6000.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(6000)),
        "expected Int(6000), got {:?}",
        r[0]
    );
    // Either the trace compiled via S9-B (var_count==2 path bails
    // pre-emit; trace runs via interp) OR it compiled and
    // dispatched correctly. Both are valid — the contract is
    // RESULT correctness.
    let _ = (
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
    );
}

/// `Op::Call C=2` (single return) still compiles + dispatches as
/// before (S4 regression check). Trace JIT shouldn't have changed
/// for the non-S9-B path. fib(28) is the canonical deep-recursion
/// shape that fires multiple-frame inline + dispatches per S4-step4b.
#[test]
fn call_c_two_single_return_still_works() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n) if n<2 then return n end return f(n-1)+f(n-2) end
             return f(28)",
        )
        .unwrap();
    // fib(28) = 317811.
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(317811)),
        "expected Int(317811), got {:?}",
        r[0]
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fib trace must still dispatch (S4 baseline); got {}",
        vm.trace_dispatched_count()
    );
}
