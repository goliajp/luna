//! v2.0 Track-R R3d — multi-way caller-pc guard + `dispatchable=true`
//! lift regression pin.
//!
//! R3d turns R3c's single-CMP guard into a chain of `icmp(Equal,
//! saved_pc, iconst(candidate_pc)) + brif(eq, stitch, next)`
//! predicates, one per distinct `caller_pc` collected from the
//! recorder's `rec.retfs` side-channel (filtered to retfs whose
//! `proto` matches the close marker's `target_proto`). When >= 2
//! distinct candidates are collected, the lowerer lifts
//! `dispatchable = true` so the primary dispatcher arm
//! (`find(|t| t.dispatchable)`) hits the trace directly without
//! going through R3c's `downrec_link.is_some()` fallback admit
//! clause.
//!
//! Workload: fib(3) called in a hot loop — same shape as R3b/R3c.
//! fib body has two call sites (`return fib(n-1) + fib(n-2)`), so
//! the recorder pushes retfs at two distinct caller_pcs. R3d's
//! dedupe collects them → multi-way guard fires → lift triggers.
//!
//! Success criterion (brief Sub 6):
//! - `multi_way_guard_emitted >= 1` — at least one trace had the
//!   lowerer's multi-way arm collect >= 2 distinct candidates and
//!   lift `dispatchable=true`.
//! - `downrec_dispatched + downrec_deopt > 0` AND
//!   `downrec_dispatched > 0` — the dispatcher actually saw the
//!   guard fire (HIT >= 1).
//! - Target inversion check: `downrec_dispatched / (downrec_dispatched
//!   + downrec_deopt) >= 0.70` (per task spec brief — multi-way is
//!   expected to flip the 90% miss-rate to >= 70% hit-rate).
//!   IF inversion fails (rate < 70%), the test records actual rates
//!   via `eprintln!` (so the run still surfaces them in `cargo test
//!   -- --nocapture`) and asserts only the weaker
//!   `downrec_dispatched > downrec_deopt` invariant (per task spec
//!   relax — "any positive gain is success").
//! - fib(3) hot loop returns 400 — correctness regression-free.

use luna_jit::version::LuaVersion;

/// fib(3) called in a hot loop. Mirrors R3c's primary test workload.
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

/// R3d primary pin: lowerer emitted a multi-way guard + lifted
/// `dispatchable=true` at least once.
#[test]
fn p16_on_fib_3_hot_loop_lowerer_emits_multi_way_guard() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_loop_r3d_emit")
        .expect("loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs (cycle-safety held across lift)");
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(3) loop returned an unexpected Value: {:?}", other),
    };
    assert_eq!(
        returned, EXPECTED_FIB_3_HOT_LOOP_SUM,
        "R3d regression — fib(3) loop sum must stay \
         {EXPECTED_FIB_3_HOT_LOOP_SUM} despite the multi-way \
         dispatchable=true lift"
    );

    let downrec_link_compiled = vm.trace_downrec_link_compiled_count();
    assert!(
        downrec_link_compiled >= 1,
        "R3a/R3b precondition — at least one trace with \
         downrec_link = Some(_) must compile (got \
         {downrec_link_compiled})"
    );

    let multi_way = vm.trace_multi_way_guard_emitted_count();
    assert!(
        multi_way >= 1,
        "R3d primary contract — multi_way_guard_emitted must be \
         >= 1 (got {multi_way}); fib(3) body's 2 call sites should \
         yield >= 2 distinct candidates per close, triggering the \
         lift. If 0, either the lowerer's dedupe captured only 1 \
         candidate (rec.retfs may not match target_proto for the \
         second site) or the lift gate misfired."
    );
}

/// R3d hit-rate inversion pin: with the multi-way guard, the
/// `downrec_dispatched / (downrec_dispatched + downrec_deopt)` ratio
/// should flip from R3c's 10% (90% miss) up to >= 70% (per task spec
/// target). The test records actual numbers + relaxes to "any
/// positive net gain" per task spec brief on stuck/blocked clauses.
#[test]
fn p16_on_fib_3_hot_loop_multi_way_guard_inverts_hit_rate() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_loop_r3d_invert")
        .expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    let dispatched = vm.trace_downrec_dispatched_count();
    let deopt = vm.trace_downrec_deopt_count();
    let total = dispatched + deopt;
    assert!(
        total > 0,
        "R3d contract — at least one downrec admit must classify a \
         trace return (dispatched={dispatched}, deopt={deopt}). \
         If both are zero, the dispatcher's admit predicate didn't \
         match — multi-way may have lifted but the trace never hit \
         the dispatch path."
    );

    // Always surface the actual rate for the verdict doc + future
    // bench tuning. Visible with `cargo test -- --nocapture`.
    let hit_rate = dispatched as f64 / total as f64;
    eprintln!(
        "R3d hit-rate: dispatched={dispatched} deopt={deopt} \
         total={total} hit_rate={hit_rate:.3}"
    );

    // Per task spec stuck-clause: "any positive gain is success".
    // We assert the weaker `dispatched > deopt` invariant (which
    // would still be a major win vs R3c's 10% hit-rate); the
    // stronger `>= 0.70` target is captured in the eprintln above
    // for verdict measurement.
    assert!(
        dispatched > deopt,
        "R3d hit-rate inversion — multi-way guard should make \
         dispatched > deopt (got dispatched={dispatched} \
         vs deopt={deopt}). R3c baseline had deopt at 90% of \
         total; R3d's multi-way should invert at least to \
         > 50%. If this fails, the multi-way dedupe likely \
         missed a hot caller_pc; bump DOWNREC_MULTI_WAY_GUARD_MAX \
         or audit the recorder's `caller_pc` capture."
    );
}

/// R3d no-regression pin for the p16-off (default) path. R3d must
/// not bump `multi_way_guard_emitted` on a workload with the cycle
/// catch off.
#[test]
fn p16_off_fib_28_no_multi_way_guard_emit() {
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

    let cl = vm.load(FIB_28_SRC, b"=fib28_r3d_p16off").expect("loads");
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
        "fib(28) p16-off must stay {EXPECTED_FIB_28} after R3d lands"
    );

    let multi_way = vm.trace_multi_way_guard_emitted_count();
    assert_eq!(
        multi_way, 0,
        "p16-off must not bump multi_way_guard_emitted, got {multi_way}"
    );
}
