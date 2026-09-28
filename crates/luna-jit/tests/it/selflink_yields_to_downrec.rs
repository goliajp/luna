//! Recorder SelfLink relax regression pin.
//!
//! The recorder lifts the SelfLink → DownRec close-cause routing at the
//! recorder's SelfLink trip site (`crates/luna-core/src/vm/exec.rs`
//! around line 5650) when `cur_depth >= 2` AND a parent Op::Call
//! ancestor exists in `rec.ops` at depth `cur_depth - 1`. The
//! lifted shape synthesises a `DownRecClose` marker from the
//! ancestor (return_pc = call.pc + 1, target_proto = call.proto,
//! depth_delta = 1) and bumps the `"selflink-yields-to-downrec"`
//! close-cause label.
//!
//! Test purposes:
//!   1. Positive — fib(28) self-link on bumps `selflink-yields-to-downrec`
//!      AND result stays 317_811.
//!   2. Negative — fib(28) self-link off does NOT bump the new label (the
//!      relax is gated on `self_link_enabled` via the SelfLink
//!      trip predicate that produces `self_link_trip`).
//!   3. Hot-loop sanity — fib(3) hot loop (the existing DownRec
//!      fixture) keeps the `downrec-restart` label and the
//!      dispatcher hit-rate counter (`downrec_dispatched +
//!      downrec_deopt > 0`) untouched. The relax must not regress
//!      the HIT-classifier behaviour on the
//!      shorter-recursion fixture that DOESN'T go through the
//!      SelfLink trip site.

use luna_jit::version::LuaVersion;

const FIB_28_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
";
const EXPECTED_FIB_28: i64 = 317_811;

const FIB_3_HOT_LOOP_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    local s = 0
    for i = 1, 200 do s = s + fib(3) end
    return s
";
const EXPECTED_FIB_3_HOT_SUM: i64 = 200 * 2; // fib(3) = 2

/// Positive pin: fib(28) self-link on bumps the new
/// `"selflink-yields-to-downrec"` close-cause label exactly once
/// (one SelfLink trip at cur_depth = 3 entry), AND the result
/// stays at 317_811.
#[test]
fn self_link_on_fib_28_bumps_selflink_yields_to_downrec_close_cause() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_28_SRC, b"=fib28_sub0_pos")
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
        "correctness regression — fib(28) must stay {EXPECTED_FIB_28} \
         on self-link on under SelfLink-to-DownRec routing"
    );

    let counts = vm.trace_close_cause_counts();
    let yields = counts
        .get("selflink-yields-to-downrec")
        .copied()
        .unwrap_or(0);
    assert!(
        yields >= 1,
        "SelfLink-to-DownRec pin — fib(28) self-link on must bump \
         \"selflink-yields-to-downrec\" >= 1 (the SelfLink trip at \
         cur_depth=3 entry reroutes to downrec_close). Got {yields} \
         (full counts: {counts:?})"
    );
    // The `self-link-retf-r1` label MUST NOT fire on fib(28):
    // every SelfLink trip on this shape satisfies cur_depth >= 2
    // AND a parent Op::Call ancestor exists.
    let retf_label = counts.get("self-link-retf-r1").copied().unwrap_or(0);
    assert_eq!(
        retf_label, 0,
        "SelfLink-to-DownRec routing retired the self-link-retf-r1 path for fib(28). \
         Got {retf_label} (full counts: {counts:?})"
    );
}

/// Negative pin: with `self_link_enabled = false` (ship default),
/// the SelfLink trip never fires, so the lift label stays at 0.
/// fib(28) result still correct via the existing trace JIT path.
#[test]
fn self_link_off_fib_28_does_not_bump_selflink_yields_to_downrec() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(false);
    vm.open_base();

    let cl = vm
        .load(FIB_28_SRC, b"=fib28_stitch_pending_neg")
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
        "fib(28) self-link off must stay {EXPECTED_FIB_28}"
    );

    let counts = vm.trace_close_cause_counts();
    let yields = counts
        .get("selflink-yields-to-downrec")
        .copied()
        .unwrap_or(0);
    assert_eq!(
        yields, 0,
        "The SelfLink relax is gated on the SelfLink trip predicate which \
         requires self-link to be enabled. Got {yields} with \
         self-link off (full counts: {counts:?})"
    );
}

/// Hot-loop sanity: fib(3) hot loop (the existing DownRec
/// fixture) keeps its `"downrec-restart"` label bumped via the
/// depth>0 Op::Return path AND the dispatcher dispatch/deopt
/// counters stay non-zero. The relax must NOT regress this
/// behaviour on the shorter-recursion fixture
/// that DOESN'T touch the SelfLink trip site.
#[test]
fn self_link_on_fib_3_hot_loop_preserves_downrec_restart_and_dispatch() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_hot_sub0_sanity")
        .expect("fib(3) hot loop loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("fib(3) hot loop runs");

    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(3) hot loop returned an unexpected Value: {:?}", other),
    };
    assert_eq!(
        returned, EXPECTED_FIB_3_HOT_SUM,
        "DownRec fixture correctness — fib(3) hot loop sum must stay \
         {EXPECTED_FIB_3_HOT_SUM} under SelfLink-to-DownRec routing"
    );

    let counts = vm.trace_close_cause_counts();
    let downrec_restart = counts.get("downrec-restart").copied().unwrap_or(0);
    assert!(
        downrec_restart >= 1,
        "DownRec recorder regression — fib(3) hot loop must still bump \
         \"downrec-restart\" via the depth>0 Op::Return path. \
         Got {downrec_restart} (full counts: {counts:?})"
    );

    let dispatched = vm.trace_downrec_dispatched_count();
    let deopt = vm.trace_downrec_deopt_count();
    assert!(
        dispatched + deopt >= 1,
        "DownRec dispatcher regression — fib(3) hot loop must still produce \
         downrec_dispatched + downrec_deopt >= 1. Got \
         dispatched={dispatched}, deopt={deopt}"
    );
}
