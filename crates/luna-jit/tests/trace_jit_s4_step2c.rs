//! P12-S4-step2c — make fib's GetUpval-touching trace dispatchable.
//!
//! Step 2b compiled fib's trace but pinned `dispatchable = false`
//! because the GetUpval's result tag was statically unknown. Step 2c
//! introduces:
//!
//! - `ExitTag::Closure` + `RegKind::Closure` variants
//! - `infer_upval_exit`: forward-walks ops after a GetUpval; if a
//!   later `Op::Call` uses R[A] as its function target before any op
//!   overwrites R[A], the upval must be a closure → ExitTag::Closure
//! - per-side-exit `exit_tags` on `CompiledTrace.per_exit_tags`:
//!   snapshots `current_kinds` at each Lt/Le/Eq+Jmp side-exit so
//!   exits firing **before** the GetUpval restore the affected slot
//!   as `Untouched` (carry entry tag) instead of pack-as-Closure
//!   with a stale Nil payload (the bug r() hit without per-exit tags)
//!
//! Result for fib: trace closes + compiles + **dispatches** with
//! correct semantics across base case (cmp side-exits) and recursive
//! paths.

use luna_jit::version::LuaVersion;

/// fib(12) under trace_jit_enabled compiles the GetUpval-touching
/// trace (step 2b) and step 2c keeps it dispatchable in principle
/// — but the length-gate (`MIN_DISPATCHABLE_TRUNC_BODY = 20`) gates
/// fib's ~7-op truncated body off the dispatch path to avoid the
/// per-dispatch overhead exceeding the prefix savings (measured
/// 1.8× slowdown without the gate). Step 3's inline emit will push
/// the body past the gate and unlock real perf.
///
/// Result must remain 144 either way (trace is cached but not
/// dispatched).
#[test]
fn fib_trace_compiles_correctly_under_step2c() {
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
        vm.trace_compiled_count() >= 1,
        "fib's trace must compile; got compiled={}",
        vm.trace_compiled_count()
    );
    // P12-S4-step4b-C-2 lifted the length-gate for inline traces —
    // fib now dispatches via the frame-mat helper at cmp@d>0 side-
    // exits. Each dispatch tears through multiple recursion levels
    // before returning to the interp.
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fib's trace dispatches via inline emit (step4b-C-2). got dispatched={}",
        vm.trace_dispatched_count()
    );
}

/// Per-side-exit `exit_tags` regression test. A helper that mirrors
/// fib's shape (`if n == 0 ... else return 1 + r(n-1)`) used to panic
/// at the Lt/Eq side-exit's restore because the trace's clean-tail
/// `exit_tags[R_getupval]` was `Closure` but the side-exit fired
/// before GetUpval ever wrote R[A], leaving the slot at its entry
/// Nil value → pack(CLOSURE, 0) → null Gc → panic. Step 2c snapshots
/// per-exit kinds so side-exits use `Untouched` for un-touched slots.
#[test]
fn early_side_exit_with_later_getupval_restores_safely() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // 6-level chain reaches MAX_INLINE_DEPTH on inner recursion; the
    // for-loop fires many calls so the trace fires + dispatches and
    // we hit the early Eq side-exit (n == 0 base case) many times.
    let r = vm
        .eval(
            "local function r(n) if n == 0 then return 1 end return 1 + r(n-1) end
             local s = 0
             for i = 1, 200 do s = s + r(6) end
             return s",
        )
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(1400)));
    // Length-gate covers r's body too (similar shape to fib).
    // Step 3 lifts. The KEY assertion here is "no panic" — the
    // result above being correct proves the run completed safely.
}
