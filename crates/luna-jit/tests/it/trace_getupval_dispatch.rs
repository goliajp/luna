//! Make fib's GetUpval-touching trace dispatchable.
//!
//! A GetUpval's result tag is not statically known from B alone. The
//! trace JIT uses:
//!
//! - `ExitTag::Closure` + `RegKind::Closure` variants
//! - `infer_upval_exit`: forward-walks ops after a GetUpval; if a
//!   later `Op::Call` uses R[A] as its function target before any op
//!   overwrites R[A], the upval must be a closure → ExitTag::Closure
//! - per-side-exit `exit_tags` on `CompiledTrace.per_exit_tags`:
//!   snapshots `current_kinds` at each Lt/Le/Eq+Jmp side-exit so
//!   exits firing **before** the GetUpval restore the affected slot
//!   as `Untouched` (carry entry tag) instead of pack-as-Closure
//!   with a stale Nil payload
//!
//! Result for fib: trace closes + compiles + **dispatches** with
//! correct semantics across base case (cmp side-exits) and recursive
//! paths.

use luna_jit::version::LuaVersion;

/// fib(12) under trace_jit_enabled compiles the GetUpval-touching
/// trace and dispatches it. The length-gate
/// (`MIN_DISPATCHABLE_TRUNC_BODY = 20`) keeps short truncated bodies
/// off the dispatch path because the per-dispatch overhead exceeds
/// the prefix savings (measured 1.8× slowdown without the gate); it
/// is skipped for inline traces like fib's.
///
/// Result must remain 144.
#[test]
fn fib_trace_compiles_and_dispatches_correctly() {
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
    // The length-gate is skipped for inline traces — fib dispatches
    // via the frame-mat helper at cmp@d>0 side-exits. Each dispatch
    // tears through multiple recursion levels before returning to the
    // interp.
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fib's trace dispatches via inline emit. got dispatched={}",
        vm.trace_dispatched_count()
    );
}

/// Per-side-exit `exit_tags` regression test. A helper that mirrors
/// fib's shape (`if n == 0 ... else return 1 + r(n-1)`) would panic
/// at the Lt/Eq side-exit's restore if the side-exit used the trace's
/// clean-tail `exit_tags[R_getupval]` (`Closure`): the side-exit fires
/// before GetUpval ever writes R[A], leaving the slot at its entry
/// Nil value → pack(CLOSURE, 0) → null Gc → panic. Per-exit kind
/// snapshots make side-exits use `Untouched` for un-touched slots.
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
    // The KEY assertion here is "no panic" — the
    // result above being correct proves the run completed safely.
}

/// An upvalue called as a function need not be a Lua closure. A trace of
/// a recursive function calling a table with `__call` (or a native) through
/// an upvalue must not restore that value under the closure tag when it
/// exits at the call; the interpreter would then run the table as a closure.
#[test]
fn upvalue_call_target_that_is_not_a_closure_keeps_its_type() {
    let cases = [
        "local hits = 0
         local MT = {}
         MT.__call = function(_, y) hits = hits + 1 return (tonumber(y) or 0) + 1 end
         local M = setmetatable({}, MT)
         local function rec(r, acc)
           if r <= 0 then return acc end
           return rec(r - 1, acc) + ((M(0.5)) + 92)
         end
         for i = 1, 40 do rec(1, 0) end
         return hits",
        "local f = math.abs
         local hits = 0
         local function rec(r, acc)
           if r <= 0 then return acc end
           return rec(r - 1, acc) + ((f(-1.5)) + 92)
         end
         for i = 1, 40 do rec(1, 0) hits = hits + 1 end
         return hits",
    ];
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        for src in cases {
            let mut vm = luna_jit::new_with_jit(v);
            vm.jit.trace_hot_threshold = 7;
            vm.jit.call_hot_threshold = 7;
            let r = vm.eval(src).unwrap();
            let hits = match r[0] {
                luna_jit::runtime::Value::Int(i) => i as f64,
                luna_jit::runtime::Value::Float(f) => f,
                ref o => panic!("{v:?}: not a number: {o:?}"),
            };
            assert_eq!(hits, 40.0, "{v:?}");
        }
    }
}
