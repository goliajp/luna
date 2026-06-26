//! v2.0 Track-R R3c — dispatcher stitch + cycle safety regression
//! pin.
//!
//! R3c wires the dispatcher consumer for the DOWNREC sentinel R3b
//! plants at the trace's tail. Pieces landed in this batch:
//!
//! 1. **Admit predicate** (`crates/luna-core/src/vm/exec.rs:6231`) —
//!    the primary `find(|t| t.head_pc == pc && t.dispatchable)` gains
//!    an OR'd `(t.downrec_link.is_some() && !suppress)` clause so
//!    R3b's `dispatchable=false` traces with a populated
//!    `downrec_link` actually enter the dispatcher. R3c keeps
//!    `dispatchable=false` per task spec (R3d lifts).
//!
//! 2. **Saved-PC slot** (`exec.rs:6294` + `trace.rs:7235`) — the
//!    dispatcher's `is_downrec_entry` arm sizes `reg_state` to
//!    `window_size + 1` and writes the parent frame's `pc` into
//!    the extra slot; the lowerer's R3b guard load now reads it
//!    from `reg_state[window_size * 8]` (was R3b's `iconst(0)`
//!    placeholder, cranelift constant-folded to always-false).
//!
//! 3. **Sentinel post-invoke arm** (`exec.rs:6423+`) — classifies
//!    the trace's return as HIT (DOWNREC sentinel) or MISS (any
//!    other sentinel / pending_err). HIT bumps
//!    `JitCounters.downrec_dispatched` and decrements the
//!    `stitch_depth_remaining` cycle budget; MISS bumps
//!    `downrec_deopt`. Both set `suppress_downrec_admit_once` so
//!    the next interpreter loop iteration skips the downrec admit
//!    and runs the natural op at `head_pc`, advancing `pc` past it.
//!
//! 4. **Stitch-cycle safety** — `JitState::STITCH_DEPTH_DEFAULT = 1`
//!    is the per-natural-entry HIT budget. The one-shot suppress
//!    flag ensures forced deopt translates into one interp tick
//!    before re-admit; the budget reset on exhaustion means later
//!    natural entries re-arm. No `entry_fn` recursion path is
//!    introduced — the lift to "tail-call into the resolved target
//!    trace via the dispatcher checkpoint" is R3d's job.
//!
//! Workload: fib(3) called in a hot loop (R3a verdict §5 chose this
//! shape because fib(28) closes via SelfLink before DownRec can fire
//! — base-case Returns never reach the recorder on the deep shape).
//! fib(3) has 5 recursive calls + 2 base-case Returns, so retfs
//! accumulate; the 3rd retf targeting fib's proto trips the
//! threshold and R3a/R3b/R3c machinery runs end-to-end.
//!
//! Success criterion (brief Sub 7):
//! - `downrec_dispatched > 0 OR downrec_deopt > 0` — at least one
//!   downrec admit happened, the trace ran, and R3c classified its
//!   return.
//! - `dispatchable == false` still set on the downrec trace —
//!   R3b/R3c pin held.
//! - fib(3) hot loop returns 400 — correctness regression-free.
//! - No infinite-loop on dispatch (test completes in finite time).

use luna_jit::version::LuaVersion;

/// fib(3) called in a hot loop — same shape as R3b's primary test.
/// fib(3) recurses 5× total, depth ≤ 2 → no SelfLink trip
/// (`RECUNROLL_THRESHOLD = 2` requires count > 2). Base cases reach
/// the recorder, depth>0 Returns push RetfRecords, the 3rd push
/// targeting fib's proto trips the threshold and the recorder stamps
/// `downrec_close`. 200 outer iters × 5 inner calls drives the hot
/// counter past the 64 threshold so the recorder fires.
const FIB_3_HOT_LOOP_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    local s = 0
    for i = 1, 200 do s = s + fib(3) end
    return s
";

/// fib(3) = 2; 200 iters × 2 = 400.
const EXPECTED_FIB_3_HOT_LOOP_SUM: i64 = 400;

/// R3c primary pin: the dispatcher's downrec admit fires + classifies
/// the trace's return + the stitch-cycle checkpoint keeps the loop
/// bounded (test completes without timing out).
#[test]
fn p16_on_fib_3_hot_loop_dispatcher_classifies_downrec_return() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_loop_r3c")
        .expect("fib(3) loop loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("fib(3) loop runs (no infinite loop = cycle safety held)");

    // 1. Result correctness — R3c's dispatcher admit + classify +
    //    deopt path must not corrupt the result. This also pins the
    //    cycle-safety design: if `suppress_downrec_admit_once` failed
    //    to break the admit loop, fib(3) hot loop would never return
    //    and the test runner would time out (not produce a wrong
    //    answer).
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(3) loop returned an unexpected Value: {:?}", other),
    };
    assert_eq!(
        returned, EXPECTED_FIB_3_HOT_LOOP_SUM,
        "R3c regression — fib(3) loop sum must stay \
         {EXPECTED_FIB_3_HOT_LOOP_SUM} despite the dispatcher \
         admitting `downrec_link`-bearing traces"
    );

    // 2. R3a/R3b precondition: the downrec lowerer scaffold actually
    //    landed a trace in the cache. If this fails, R3c's admit can
    //    never fire (no `downrec_link.is_some()` trace to admit).
    let downrec_link_compiled = vm.trace_downrec_link_compiled_count();
    assert!(
        downrec_link_compiled >= 1,
        "R3a/R3b precondition — fib(3) hot loop must compile at \
         least one trace with downrec_link = Some(_), got \
         {downrec_link_compiled}"
    );

    // 3. R3c main contract: the dispatcher actually admitted +
    //    classified at least one downrec trace return. HIT or MISS
    //    both count — the saved-PC mapping (parent frame's pc) may
    //    coincidentally match the recorded `dr_return_pc` (HIT) or
    //    miss (MISS); either way R3c bumps a counter.
    let downrec_dispatched = vm.trace_downrec_dispatched_count();
    let downrec_deopt = vm.trace_downrec_deopt_count();
    assert!(
        downrec_dispatched > 0 || downrec_deopt > 0,
        "R3c contract — at least one downrec admit must classify a \
         trace return (dispatched={downrec_dispatched}, \
         deopt={downrec_deopt}). If both are zero, either the admit \
         predicate didn't fire (downrec_link trace never matched a \
         dispatch pc) or the post-invoke arm misclassified the \
         return shape."
    );
}

/// R3c cycle-safety pin: fib(3) hot loop with p16-on completes in
/// finite time. If `suppress_downrec_admit_once` or the
/// `stitch_depth_remaining` budget failed, the admit loop would
/// run forever and Cargo's test timeout would catch it.
///
/// This is a separate test (rather than just relying on the primary)
/// because Cargo runs each `#[test]` with its own watchdog; an
/// infinite-loop in the primary would fail the test runner without
/// surfacing the specific assertion that broke.
#[test]
fn p16_on_fib_3_hot_loop_no_infinite_dispatch_cycle() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_loop_r3c_cycle")
        .expect("loads");
    // If this `call_value` doesn't return, the dispatcher admit loop
    // is unbounded — R3c's stitch-cycle safety failed.
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("cycle-safety held");
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(3) loop returned an unexpected Value: {:?}", other),
    };
    assert_eq!(returned, EXPECTED_FIB_3_HOT_LOOP_SUM);
}

/// R3b regression pin: R3c does NOT lift `dispatchable=true`. The
/// downrec trace stays pinned with `dispatch_off_reason =
/// "downrec-stitch-pending"` (set by R3b's lowerer arm) — the R3c
/// admit predicate's OR'd `downrec_link.is_some()` arm is the only
/// way the trace enters the dispatcher.
#[test]
fn p16_on_fib_3_hot_loop_downrec_trace_stays_dispatchable_false() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_loop_r3c_pin")
        .expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    // The "downrec-stitch-pending" label is set by R3b's lowerer
    // arm together with `dispatchable=false` on the same trace.
    // R3c does NOT clear this — the lift is R3d's job.
    let reasons = vm.trace_dispatch_off_reasons();
    assert!(
        reasons.contains(&"downrec-stitch-pending"),
        "R3c must not clear R3b's downrec-stitch-pending label — \
         dispatch_off_reasons = {reasons:?}"
    );
}

/// p16-off gate check: with the cycle catch + recorder DownRec gate
/// off, the dispatcher admit's downrec arm never fires. fib(28)
/// returns correctly via interp + chunk-JIT paths. Pins R3c's
/// "no silent behavior change on p16-off path" invariant.
#[test]
fn p16_off_fib_28_no_downrec_dispatch_counters_bump() {
    const FIB_28_SRC: &[u8] = b"
        local function fib(n)
            if n < 2 then return n end
            return fib(n - 1) + fib(n - 2)
        end
        return fib(28)
    ";
    const EXPECTED_FIB_28: i64 = 317_811;

    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(false);
    vm.open_base();

    let cl = vm.load(FIB_28_SRC, b"=fib28_r3c_p16off").expect("loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(28) returned an unexpected Value: {:?}", other),
    };
    assert_eq!(
        returned, EXPECTED_FIB_28,
        "fib(28) p16-off must stay {EXPECTED_FIB_28} after R3c lands"
    );

    // Both R3c counters must stay zero — no downrec admit can fire
    // when p16 (and thus the recorder's DownRec close gate) is off.
    let dispatched = vm.trace_downrec_dispatched_count();
    let deopt = vm.trace_downrec_deopt_count();
    assert_eq!(
        dispatched, 0,
        "p16-off must not bump downrec_dispatched, got {dispatched}"
    );
    assert_eq!(
        deopt, 0,
        "p16-off must not bump downrec_deopt, got {deopt}"
    );
}
