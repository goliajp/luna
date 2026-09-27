//! Close-cause hygiene regression pin.
//!
//! Recorder-side and lowerer-side close causes share one per-reason
//! bucket, `JitCounters::close_cause_counts`, fed by the helper
//! `bump_close_cause`, so probes count by reason in O(1) instead of
//! walking the ordered `dispatch_off_reasons` Vec.
//!
//! Tests below pin:
//! 1. `self-link-retf-r1` fires on fib(28) self-link on
//!    (lowerer-side dispatch_off mirror).
//! 2. `length-gate` fires on fib(28) self-link off
//!    (lowerer-side dispatch_off mirror, ship-default path).
//! 3. The HashMap surface and the Vec surface stay paired on the
//!    lowerer dispatch_off site.
//! 4. The accessor returns a stable HashMap reference (empty on a
//!    fresh Vm).
//!
//! `trace-overflow` (recorder MAX_TRACE_LEN) and
//! `partial-coverage-discard` (recorder partial-coverage discard) are tagged at
//! their bump sites (exec.rs) but not exercised E2E here — overflow
//! requires a Lua program > MAX_TRACE_LEN ops, and partial-coverage
//! discard requires a Proto whose call-triggered first close records
//! a strictly-shorter-than-half body (fib(28) closes via
//! `already_cached` short-circuit on subsequent calls, not via the
//! discard branch).

use luna_jit::version::LuaVersion;

const FIB_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
";

/// Pin: self-link on close-cause bucket contains the safety-pin label with
/// a non-zero count, AND the corresponding `dispatch_off_reasons` Vec
/// entry matches O(1) on the HashMap (mirrors the third test in
/// `self_link_fib_correctness.rs`, but on the HashMap surface). Asserts the HashMap-Vec pairing
/// invariant holds for whichever label the routing produced.
///
/// A SelfLink close pins `"self-link-retf-r1"` on both surfaces; when
/// `cur_depth >= 2` the recorder reroutes the SelfLink trip to
/// `downrec_close` and the lowerer's single-candidate guard chain
/// pins `"downrec-stitch-pending"`. The test accepts EITHER label,
/// picks whichever is present, and asserts the HashMap-Vec pairing
/// invariant for that label.
#[test]
fn self_link_on_fib_28_bumps_safety_pin_label_in_close_cause_counts() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_selflink_on").expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    let counts = vm.trace_close_cause_counts();
    let self_link_n = counts.get("self-link-retf-r1").copied().unwrap_or(0);
    let stitch_pending_n = counts.get("downrec-stitch-pending").copied().unwrap_or(0);
    let (label, n) = if self_link_n >= 1 {
        ("self-link-retf-r1", self_link_n)
    } else if stitch_pending_n >= 1 {
        ("downrec-stitch-pending", stitch_pending_n)
    } else {
        panic!(
            "expected close_cause_counts to contain >= 1 of \
             \"self-link-retf-r1\" (SelfLink safety pin) \
             OR \"downrec-stitch-pending\" (SelfLink-to-DownRec routing \
             single-candidate fallback); got neither (full counts: {:?}, \
             dispatch_off_reasons: {:?})",
            counts,
            vm.trace_dispatch_off_reasons(),
        );
    };
    // Pin the parallel Vec surface: bump_close_cause is invoked
    // alongside the Vec push, so the two surfaces must agree.
    let vec_hits = vm
        .trace_dispatch_off_reasons()
        .iter()
        .filter(|r| **r == label)
        .count() as u64;
    assert_eq!(
        n, vec_hits,
        "HashMap count and Vec count for {label} diverged \
         — bump_close_cause + dispatch_off_reasons.push must stay paired"
    );
}

/// Pin: self-link off (ship-default trace path) close-cause bucket contains
/// `length-gate` with a non-zero count. fib(28) under the trace JIT
/// closes ~8 recordings; ~3 of those compile but the lowerer rejects
/// them at the length-gate (their body is shorter than the
/// dispatchable-trunc minimum). The HashMap surfaces these in O(1)
/// without walking the `dispatch_off_reasons` Vec.
#[test]
fn self_link_off_fib_28_bumps_length_gate_in_close_cause_counts() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(false);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_selflink_off").expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    let counts = vm.trace_close_cause_counts();
    let n = counts.get("length-gate").copied().unwrap_or(0);
    assert!(
        n >= 1,
        "expected close_cause_counts[\"length-gate\"] >= 1 on fib(28) self-link off; \
         got {n} (full counts: {:?}, dispatch_off_reasons: {:?})",
        counts,
        vm.trace_dispatch_off_reasons(),
    );
    // Pin HashMap-Vec pairing on the most-common lowerer label.
    let vec_hits = vm
        .trace_dispatch_off_reasons()
        .iter()
        .filter(|r| **r == "length-gate")
        .count() as u64;
    assert_eq!(
        n, vec_hits,
        "HashMap count and Vec count for length-gate diverged — \
         bump_close_cause + dispatch_off_reasons.push must stay paired"
    );
}

/// Pin: the HashMap-Vec pairing invariant holds globally. Every
/// reason that appears in `dispatch_off_reasons` (Vec, ordered) must
/// appear in `close_cause_counts` (HashMap, by-reason count) with the
/// matching cardinality. This is the structural property that the
/// `bump_close_cause` helper enforces; the test pins it against
/// future drift (e.g. a new dispatch_off site that forgets to mirror).
#[test]
fn dispatch_off_reasons_vec_and_close_cause_counts_hashmap_agree() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_selflink_on_pair").expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    let counts = vm.trace_close_cause_counts();
    let reasons = vm.trace_dispatch_off_reasons();

    // For every reason in the Vec, the HashMap count must be >=
    // the Vec occurrence count. (>= rather than == because the
    // recorder-side overflow / discard labels also bump the HashMap
    // without touching the Vec — they would inflate the HashMap
    // bucket above the Vec count, which is fine.)
    use std::collections::HashMap;
    let mut vec_counts: HashMap<&'static str, u64> = HashMap::new();
    for r in reasons {
        *vec_counts.entry(*r).or_insert(0) += 1;
    }
    for (reason, vec_n) in &vec_counts {
        let map_n = counts.get(reason).copied().unwrap_or(0);
        assert!(
            map_n >= *vec_n,
            "close_cause_counts[\"{reason}\"] = {map_n} < dispatch_off Vec count {vec_n} \
             — a dispatch_off site is bumping the Vec without mirroring to the HashMap"
        );
    }
}

/// Sanity: `trace_close_cause_counts()` accessor returns a stable
/// reference to the HashMap. Pinning the surface so future refactors
/// don't break embedder probes that depend on the `&HashMap` return.
#[test]
fn trace_close_cause_counts_accessor_returns_empty_on_fresh_vm() {
    let vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    let counts: &std::collections::HashMap<&'static str, u64> = vm.trace_close_cause_counts();
    assert!(
        counts.is_empty(),
        "fresh Vm must start with no close-cause counts; got {:?}",
        counts
    );
}
