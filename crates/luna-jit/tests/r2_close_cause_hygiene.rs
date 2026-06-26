//! v2.0 Track-R R2 — close-cause hygiene regression pin.
//!
//! Pre-R2 baseline: the recorder-side overflow path bumped only the
//! flat `aborted` counter with no reason label; the partial-coverage
//! discard path bumped `closed` + `closed_lens` with no reason label;
//! the lowerer-side dispatch_off was tallied as an ordered
//! `Vec<&'static str>` that probes had to walk O(N) to count by
//! reason. The R0 verdict §5 / R1 verdict §5 R2 sketch asked for a
//! unified per-reason bucket — that's `JitCounters::close_cause_counts`
//! plus the helper `bump_close_cause`.
//!
//! Tests below pin:
//! 1. `self-link-retf-r1` fires on fib(28) p16-on
//!    (lowerer-side dispatch_off mirror).
//! 2. `length-gate` fires on fib(28) p16-off
//!    (lowerer-side dispatch_off mirror, ship-default path).
//! 3. The HashMap surface and the Vec surface stay paired on the
//!    lowerer dispatch_off site.
//! 4. The accessor returns a stable HashMap reference (empty on a
//!    fresh Vm).
//!
//! `trace-overflow` (recorder MAX_TRACE_LEN) and
//! `partial-coverage-discard` (recorder S13-I discard) are tagged at
//! their bump sites (exec.rs) but not exercised E2E here — overflow
//! requires a Lua program > MAX_TRACE_LEN ops, and partial-coverage
//! discard requires a Proto whose call-triggered first close records
//! a strictly-shorter-than-half body (fib(28) closes via
//! `already_cached` short-circuit on subsequent calls, not via the
//! discard branch). See the R2 verdict for the recorder-side
//! observability gap that future fixtures should close.

use luna_jit::version::LuaVersion;

const FIB_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
";

/// Pin: p16-on close-cause bucket contains `self-link-retf-r1` with
/// a non-zero count. The matching `dispatch_off_reason` is also
/// non-empty (mirrors R1 test 3, but on the HashMap surface that R2
/// added). Asserting both surfaces guards against future drift where
/// the Vec push and HashMap bump diverge.
#[test]
fn p16_on_fib_28_bumps_self_link_retf_r1_in_close_cause_counts() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_p16on_r2").expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    let counts = vm.trace_close_cause_counts();
    let n = counts.get("self-link-retf-r1").copied().unwrap_or(0);
    assert!(
        n >= 1,
        "expected close_cause_counts[\"self-link-retf-r1\"] >= 1; got {n} \
         (full counts: {:?}; dispatch_off_reasons: {:?})",
        counts,
        vm.trace_dispatch_off_reasons(),
    );
    // Pin the parallel Vec surface: bump_close_cause is invoked
    // alongside the Vec push, so the two surfaces must agree.
    let vec_hits = vm
        .trace_dispatch_off_reasons()
        .iter()
        .filter(|r| **r == "self-link-retf-r1")
        .count() as u64;
    assert_eq!(
        n, vec_hits,
        "HashMap count and Vec count for self-link-retf-r1 diverged \
         — bump_close_cause + dispatch_off_reasons.push must stay paired"
    );
}

/// Pin: p16-off (ship-default trace path) close-cause bucket contains
/// `length-gate` with a non-zero count. fib(28) under the trace JIT
/// closes ~8 recordings; ~3 of those compile but the lowerer rejects
/// them at the length-gate (their body is shorter than the
/// dispatchable-trunc minimum). R2 surfaces these in O(1) on the
/// HashMap; pre-R2 a probe had to walk `dispatch_off_reasons` Vec.
#[test]
fn p16_off_fib_28_bumps_length_gate_in_close_cause_counts() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(false);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_p16off_r2").expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    let counts = vm.trace_close_cause_counts();
    let n = counts.get("length-gate").copied().unwrap_or(0);
    assert!(
        n >= 1,
        "expected close_cause_counts[\"length-gate\"] >= 1 on fib(28) p16-off; \
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
/// matching cardinality. This is the structural property that the R2
/// `bump_close_cause` helper enforces; the test pins it against
/// future drift (e.g. a new dispatch_off site that forgets to mirror).
#[test]
fn dispatch_off_reasons_vec_and_close_cause_counts_hashmap_agree() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_p16on_pair").expect("loads");
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
