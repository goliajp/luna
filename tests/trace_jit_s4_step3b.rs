//! P12-S4-step3b — body emit consumes `op_offsets` + `enclosing_call_a`
//! + `window_size`. Self-recursive `Op::Call` no longer truncates the
//! trace; the emit loop shifts to the callee's register window via
//! `regs_full[off..off+max_stack]` shadowing, and `Op::Return1` at
//! depth>0 emits the copy-back into the caller's slot.
//!
//! Today the recorder still closes call-triggered traces at the first
//! self-recursive `Op::Call`'s re-entry (step 2 design — `fib` records
//! one depth-0 pass through the body), so the lowerer's new inline
//! path doesn't fire on real Lua code yet. These tests verify the
//! infrastructure is sound: fib/r still produce correct results, the
//! length-gate still pins dispatched=0 for short truncated bodies, and
//! deeper recursion levels don't panic the new offset/window machinery.
//! Step 4 will extend the recorder to walk into the callee, at which
//! point this file's tests get sibling assertions for the new dispatch
//! shape.

use luna::version::LuaVersion;
use luna::vm::Vm;

/// fib(28) under the new offset-aware emit still returns the right
/// result. Step3b's plumbing changes (window_size, op_offsets,
/// TraceEnd::InlineAbort, regs_full shadow) must not regress fib's
/// trace closure / compile / dispatch path.
#[test]
fn fib_28_correct_under_step3b_plumbing() {
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
        "fib's trace must still compile under step3b plumbing"
    );
    // P12-S4-step4b-C-2 — inline cmp@d>0 emit + helper landed; the
    // length-gate is skipped when `per_exit_inline` is non-empty, so
    // fib's trace dispatches via the inline path. Each dispatch
    // tears through multiple recursion levels via the frame-mat
    // helper.
    assert!(
        vm.trace_dispatched_count() >= 1,
        "fib(28) dispatches via inline self-rec (step4b-C-2). got dispatched={}",
        vm.trace_dispatched_count()
    );
}

/// Deep self-recursion (`r(6)` 200 times) exercises the recorder's
/// `MAX_INLINE_DEPTH` cap and the depth-aware close detection. Even
/// though step3b doesn't add new dispatch paths here, the new emit
/// loop's `regs_full` shadow and `current_kinds[off + X]` accesses
/// must not corrupt slot tracking — a regression would manifest as a
/// wrong sum or a panic during compile.
#[test]
fn deep_recursive_calls_still_produce_correct_sums() {
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
    // No `dispatched` assertion — recorder/lowerer interactions
    // around the inner self-recursive r() differ from fib's shape
    // and the gating is the focus of step4.
}

/// Plain back-edge loops (no inlining) still hit the dispatcher's
/// fast path — step3b's effective_end rewrite must keep
/// non-inline traces on the existing Op::ForLoop / cmp branches.
#[test]
fn numeric_loop_still_dispatches_after_step3b() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // Sum 1..1000 — a numeric for that closes on Op::ForLoop and
    // dispatches via the S3 path. Step3b touched the shared lowerer
    // so this must keep working.
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
        "the numeric-for trace must still dispatch through step3b's lowerer; \
         got dispatched={}",
        vm.trace_dispatched_count()
    );
}
