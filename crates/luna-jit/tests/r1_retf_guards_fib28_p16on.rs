//! v2.0 Track-R R1 — RETF-guards correctness primitive regression pin.
//!
//! Pre-R1 baseline (commit `753c972` R0 verdict): with
//! `jit.p16_self_link_enabled = true`, the P16-B SelfLink tail emitted
//! a slot-copy `regs_full[i] = regs_full[bump_off + i]` for
//! `i in 0..max_stack` + branched back to body_loop. fib's
//! non-tail-recursive body (depth-0 Subs polluting head-frame slots
//! BEFORE the recursive Call, depth>0 base-case Returns whose layout
//! doesn't match head's) miscompiled — fib(28) returned 45 instead of
//! 317_811.
//!
//! R1 replaces the slot-copy tail with `emit_store_back_and_return_pc
//! (head_pc) + dispatchable = false`. The trace still compiles but the
//! dispatcher refuses to enter it; interp runs the recursion naturally.
//!
//! This test pins the **correctness** half (result must be 317_811
//! with p16 on). The infrastructure half (`RetfRecord`s collected in
//! `TraceRecord.retfs` for R3's down-rec stitch) is verified at the
//! type level — adding `retfs: Vec::new()` to both constructors keeps
//! the workspace `cargo test --workspace --lib` green (367 pass).

use luna_jit::version::LuaVersion;

const FIB_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
";

const EXPECTED_FIB_28: i64 = 317_811;

/// The R0 diag's Row 3 setup — chunk JIT off (so the recorder sees
/// fib's call sites instead of being short-circuited by
/// `try_jit_call_op`), trace JIT on, **p16 self-link enabled**.
/// Pre-R1: returned 45. Post-R1: returned 317_811.
#[test]
fn fib_28_returns_correct_value_with_p16_self_link_enabled() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_p16on").expect("fib(28) loads");
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
        "R1 regression — pre-R1 returned 45 from corrupted SelfLink \
         snapshot-restore; post-R1 must return {EXPECTED_FIB_28}"
    );
}

/// Same as above but with p16 OFF — the ship default path. Pins that
/// R1's dispatcher-gate-by-dispatchable change didn't disturb the
/// recorder + lowerer flow on the default flag. Required because R1's
/// recorder side-channel push is gated on `p16_self_link_enabled`;
/// this test guards against regressions where the gate slips.
#[test]
fn fib_28_returns_correct_value_with_p16_self_link_disabled() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(false);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_p16off").expect("fib(28) loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("fib(28) runs");
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(28) returned an unexpected Value: {:?}", other),
    };
    assert_eq!(returned, EXPECTED_FIB_28);
}

/// Pin that the R1 dispatch_off_reason label is present when p16-on
/// fires. The diag harness reports this — surfacing it as a test
/// keeps the gate label discoverable for future probes (R2 close-cause
/// hygiene + R3 down-rec stitch will need to disambiguate this from
/// other dispatch_off reasons).
#[test]
fn p16_on_fib_28_records_self_link_retf_r1_dispatch_off_reason() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm.load(FIB_SRC, b"=fib28_p16on_label").expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    let reasons = vm.trace_dispatch_off_reasons();
    assert!(
        reasons.contains(&"self-link-retf-r1"),
        "expected at least one trace pinned dispatch_off via R1 SelfLink \
         deopt — got reasons: {:?}",
        reasons
    );
}
