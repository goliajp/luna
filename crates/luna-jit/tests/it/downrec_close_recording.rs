//! TraceEnd::DownRec recorder-emit regression pin.
//!
//! The recorder-side close marker for LuaJIT's
//! `LJ_TRLINK_DOWNREC` shape (`lj_record.c:912 lj_trace_err
//! (LJ_TRERR_DOWNREC)`). When a depth>0 `Op::Return` fires inside an
//! active recording AND the `rec.retfs` chain accumulates more than
//! `RECUNROLL_THRESHOLD` records targeting the same caller proto, the
//! recorder stamps `TraceRecord.downrec_close = Some(...)` and the
//! lowerer's `end_idx` picker routes through the
//! `TraceEnd::DownRec` arm.
//!
//! Result correctness must stay at 317_811 on fib(28). For the
//! workload that actually reaches base-case Returns during recording,
//! the `"downrec-restart"` label fires.
//!
//! **Workload split**:
//! - fib(28) self-link on closes via the **self-link cycle catch** at
//!   depth 3 entry BEFORE any base case is reached — `rec.retfs`
//!   stays empty, so the Return-side downrec catch does NOT fire
//!   (`downrec-restart` stays 0).
//! - A small-N fib (`fib(3)`) called in a hot loop (so the recorder
//!   hits its call-hot threshold) records a body that **does** reach
//!   base-case Returns. The 3rd RetfRecord targeting `fib`'s proto
//!   trips the threshold and stamps `downrec_close`.
//!
//! This test pins the scaffold: the variant exists and the recorder
//! emits on the workload shape that reaches base cases.

use luna_jit::version::LuaVersion;

const FIB_28_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
";

/// Hot-loop workload that calls `fib(3)` 200 times. fib(3) recurses
/// 5× total, depth ≤ 2 → no self-link cycle trip (`RECUNROLL_THRESHOLD
/// = 2` requires count > 2). Base cases (`n < 2 -> return n`) are
/// reached, so depth>0 Returns push RetfRecords. 200 outer iters ×
/// 5 inner calls = 1000 bumps on fib's `call_hot_count` (threshold
/// 64 = ~13 iters before the recorder fires); recording fires on a
/// later call and captures the full fib(3) recursion tree's retfs.
const FIB_3_HOT_LOOP_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    local s = 0
    for i = 1, 200 do s = s + fib(3) end
    return s
";

const EXPECTED_FIB_28: i64 = 317_811;
// fib(3) = 2; 200 iters × 2 = 400.
const EXPECTED_FIB_3_HOT_LOOP_SUM: i64 = 400;

/// Scaffold pin: on a workload where the recorder reaches
/// base-case Returns (small-N fib called in a hot loop), the
/// `"downrec-restart"` close-cause bumps at least once: RetfRecords
/// accumulate past `RECUNROLL_THRESHOLD` and stamp the marker.
#[test]
fn self_link_on_fib_3_hot_loop_bumps_downrec_restart_close_cause() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_loop")
        .expect("fib(3) loop loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("fib(3) loop runs");

    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(3) loop returned an unexpected Value: {:?}", other),
    };
    assert_eq!(
        returned, EXPECTED_FIB_3_HOT_LOOP_SUM,
        "DownRec recorder regression — fib(3) loop sum must stay {EXPECTED_FIB_3_HOT_LOOP_SUM}"
    );

    let counts = vm.trace_close_cause_counts();
    let downrec_restart = counts.get("downrec-restart").copied().unwrap_or(0);
    assert!(
        downrec_restart >= 1,
        "expected downrec-restart close-cause >= 1 on self-link on \
         fib(3) hot loop, got {downrec_restart} (full counts: {counts:?})"
    );
}

/// fib(28) self-link on routes the SelfLink trip through the
/// `downrec_close` lift (the SelfLink trip site at `cur_depth >= 2`
/// synthesises a `DownRecClose` marker from the most recent parent
/// Op::Call ancestor and bumps `"selflink-yields-to-downrec"`). The
/// end_idx picker routes through the DownRec arm; the lowerer's
/// single-candidate guard chain keeps `dispatchable=false` +
/// `"downrec-stitch-pending"` label. Pin: result correctness
/// (317_811) + `"selflink-yields-to-downrec"` >= 1 + the
/// `"self-link-retf-r1"` label does NOT fire (self-recursion at
/// cur_depth >= 2 takes the DownRec path) + the `"downrec-restart"`
/// non-trip (the depth>0 Op::Return path requires `rec.retfs`
/// non-empty, which never reaches the recorder for fib(28)).
#[test]
fn self_link_on_fib_28_selflink_yields_to_downrec() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_28_SRC, b"=fib28_no_downrec")
        .expect("fib(28) loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("fib(28) runs");

    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(28) returned an unexpected Value: {:?}", other),
    };
    assert_eq!(
        returned, EXPECTED_FIB_28,
        "regression — fib(28) must stay {EXPECTED_FIB_28} on self-link on"
    );

    let counts = vm.trace_close_cause_counts();
    let yields = counts
        .get("selflink-yields-to-downrec")
        .copied()
        .unwrap_or(0);
    assert!(
        yields >= 1,
        "SelfLink-to-DownRec pin — fib(28) self-link on must bump \
         \"selflink-yields-to-downrec\" >= 1 (SelfLink trip rerouted \
         to downrec_close at cur_depth >= 2). Got {yields} (full \
         counts: {counts:?})"
    );
    let retf_label = counts.get("self-link-retf-r1").copied().unwrap_or(0);
    assert_eq!(
        retf_label, 0,
        "SelfLink-to-DownRec routing retired the self-link-retf-r1 path for fib(28) \
         (SelfLink trip now routes through downrec_close). Got \
         {retf_label} (full counts: {counts:?})"
    );
}

/// self-link off symmetric pin: with the cycle catch + recorder gate off,
/// neither label fires. fib(28) result still correct.
#[test]
fn self_link_off_fib_28_no_downrec_restart_close_cause() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(false);
    vm.open_base();

    let cl = vm.load(FIB_28_SRC, b"=fib28_selflink_off").expect("loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(28) returned an unexpected Value: {:?}", other),
    };
    assert_eq!(returned, EXPECTED_FIB_28);

    let counts = vm.trace_close_cause_counts();
    let downrec_restart = counts.get("downrec-restart").copied().unwrap_or(0);
    assert_eq!(
        downrec_restart, 0,
        "self-link off must not bump downrec-restart — got {downrec_restart}"
    );
}
