//! v2.0 Track-R R3a — TraceEnd::DownRec recorder-emit regression pin.
//!
//! R3a adds the recorder-side close marker for LuaJIT's
//! `LJ_TRLINK_DOWNREC` shape (`lj_record.c:912 lj_trace_err
//! (LJ_TRERR_DOWNREC)`). When a depth>0 `Op::Return` fires inside an
//! active recording AND the `rec.retfs` chain accumulates more than
//! `RECUNROLL_THRESHOLD` records targeting the same caller proto, the
//! recorder stamps `TraceRecord.downrec_close = Some(...)` and the
//! lowerer's `end_idx` picker routes through the new
//! `TraceEnd::DownRec` arm.
//!
//! R3a keeps R1's safe deopt-tail (`dispatchable=false; reason =
//! "self-link-retf-r1"`). Result correctness must stay at 317_811
//! on fib(28) (R1 regression-free). For the workload that actually
//! reaches base-case Returns during recording, the `"downrec-restart"`
//! label fires.
//!
//! **Workload split**:
//! - fib(28) p16-on closes via the **self-link cycle catch** at
//!   depth 3 entry BEFORE any base case is reached — `rec.retfs`
//!   stays empty, R3a's downrec catch does NOT fire. This pins R1
//!   regression-free + R3a's downrec-restart NOT fired.
//! - A small-N fib (`fib(3)`) called in a hot loop (so the recorder
//!   hits its call-hot threshold) records a body that **does** reach
//!   base-case Returns. The 3rd RetfRecord targeting `fib`'s proto
//!   trips the threshold and stamps `downrec_close`.
//!
//! R3b lifts `dispatchable=true` via a real native back-edge (retf
//! guard + stitch sentinel). Until then this test pins the scaffold:
//! variant exists, recorder emits on the workload shape that reaches
//! base cases, R1 safe-deopt label still fires on the deep-recursion
//! shape that closes via self-link.

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

/// R3a scaffold pin: on a workload where the recorder reaches
/// base-case Returns (small-N fib called in a hot loop), the
/// `"downrec-restart"` close-cause bumps at least once. Without
/// R3a's catch, the count is 0; with R3a, RetfRecords accumulate
/// past `RECUNROLL_THRESHOLD` and stamp the marker.
#[test]
fn p16_on_fib_3_hot_loop_bumps_downrec_restart_close_cause() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_loop_r3a")
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
        "R3a regression — fib(3) loop sum must stay {EXPECTED_FIB_3_HOT_LOOP_SUM}"
    );

    let counts = vm.trace_close_cause_counts();
    let downrec_restart = counts.get("downrec-restart").copied().unwrap_or(0);
    assert!(
        downrec_restart >= 1,
        "R3a expected downrec-restart close-cause >= 1 on p16-on \
         fib(3) hot loop, got {downrec_restart} (full counts: {counts:?})"
    );
}

/// fib(28) p16-on routes the SelfLink trip through R3.3+ sub-0's
/// `downrec_close` lift (the SelfLink trip site at `cur_depth >= 2`
/// synthesises a `DownRecClose` marker from the most recent parent
/// Op::Call ancestor and bumps `"selflink-yields-to-downrec"`). The
/// end_idx picker routes through the DownRec arm; the lowerer's R3d
/// single-candidate guard chain keeps `dispatchable=false` +
/// `"downrec-stitch-pending"` label. Pin: R1 result correctness
/// (317_811) + `"selflink-yields-to-downrec"` >= 1 + the
/// `"self-link-retf-r1"` label NO LONGER fires (sub-0 retired this
/// path for self-recursion at cur_depth >= 2).
///
/// v2.0 Track-R R3.3+ sub-0 migration: pre-sub-0 this test asserted
/// the OPPOSITE invariant — that fib(28)'s SelfLink-vs-DownRec catch
/// fell on the SelfLink side. Sub-0 changes the recorder routing so
/// fib(28) reaches DownRec via the SelfLink-yields lift; the test
/// migrates to the new invariant while keeping the R1 correctness
/// pin and the `"downrec-restart"` non-trip (the depth>0 Op::Return
/// path still requires `rec.retfs` non-empty, which never reaches
/// the recorder for fib(28)).
#[test]
fn p16_on_fib_28_selflink_yields_to_downrec() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_28_SRC, b"=fib28_r3a_no_downrec")
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
        "R1 regression — fib(28) must stay {EXPECTED_FIB_28} on p16-on"
    );

    let counts = vm.trace_close_cause_counts();
    let yields = counts
        .get("selflink-yields-to-downrec")
        .copied()
        .unwrap_or(0);
    assert!(
        yields >= 1,
        "R3.3+ sub-0 pin — fib(28) p16-on must bump \
         \"selflink-yields-to-downrec\" >= 1 (SelfLink trip rerouted \
         to downrec_close at cur_depth >= 2). Got {yields} (full \
         counts: {counts:?})"
    );
    let r1_label = counts.get("self-link-retf-r1").copied().unwrap_or(0);
    assert_eq!(
        r1_label, 0,
        "R3.3+ sub-0 retired the self-link-retf-r1 path for fib(28) \
         (SelfLink trip now routes through downrec_close). Got \
         {r1_label} (full counts: {counts:?})"
    );
}

/// p16-OFF symmetric pin: with the cycle catch + recorder gate off,
/// neither label fires. fib(28) result still correct.
#[test]
fn p16_off_fib_28_no_downrec_restart_close_cause() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(false);
    vm.open_base();

    let cl = vm.load(FIB_28_SRC, b"=fib28_r3a_p16off").expect("loads");
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
        "p16-off must not bump downrec-restart — got {downrec_restart}"
    );
}
