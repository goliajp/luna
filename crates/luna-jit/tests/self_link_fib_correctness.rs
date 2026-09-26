//! RETF-guards correctness regression pin.
//!
//! With `jit.self_link_enabled = true`, a SelfLink tail that emits
//! a slot-copy `regs_full[i] = regs_full[bump_off + i]` for
//! `i in 0..max_stack` + branches back to body_loop miscompiles fib's
//! non-tail-recursive body (depth-0 Subs polluting head-frame slots
//! BEFORE the recursive Call, depth>0 base-case Returns whose layout
//! doesn't match head's) — fib(28) returns 45 instead of 317_811.
//!
//! The SelfLink tail therefore emits `emit_store_back_and_return_pc
//! (head_pc) + dispatchable = false`. The trace still compiles but the
//! dispatcher refuses to enter it; interp runs the recursion naturally.
//!
//! This test pins the **correctness** half (result must be 317_811
//! with self-link on). The infrastructure half (`RetfRecord`s collected in
//! `TraceRecord.retfs` for the down-rec stitch) is verified at the
//! type level.

use luna_jit::version::LuaVersion;

const FIB_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
";

const EXPECTED_FIB_28: i64 = 317_811;

/// Chunk JIT off (so the recorder sees
/// fib's call sites instead of being short-circuited by
/// `try_jit_call_op`), trace JIT on, **self-link enabled**.
/// The slot-copy tail returned 45; the correct result is 317_811.
#[test]
fn fib_28_returns_correct_value_with_self_link_enabled() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_SRC, b"=fib28_selflink_on")
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
        "regression — the slot-copy SelfLink tail returned 45 from corrupted \
         snapshot-restore; it must return {EXPECTED_FIB_28}"
    );
}

/// Same as above but with self-link OFF — the ship default path. Pins that
/// the dispatcher-gate-by-dispatchable doesn't disturb the
/// recorder + lowerer flow on the default flag. Required because the
/// recorder side-channel push is gated on `self_link_enabled`;
/// this test guards against regressions where the gate slips.
#[test]
fn fib_28_returns_correct_value_with_self_link_disabled() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(false);
    vm.open_base();

    let cl = vm
        .load(FIB_SRC, b"=fib28_selflink_off")
        .expect("fib(28) loads");
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

/// Pin that the dispatch_off_reason label produced by the safety pin
/// for fib(28) self-link on is visible at the diag-equivalent probe surface.
///
/// A SelfLink close pins `dispatch_off_reason = "self-link-retf-r1"`;
/// when `cur_depth >= 2` the recorder reroutes the SelfLink trip to
/// `downrec_close` and the lowerer's single-candidate guard chain pins
/// `dispatch_off_reason = "downrec-stitch-pending"`. Both labels reach
/// the SAME functional outcome (trace compiles non-dispatchable +
/// interp runs naturally → result 317811). The test accepts EITHER
/// label while still pinning "at least one trace was prevented from
/// dispatching on self-link on".
#[test]
fn self_link_on_fib_28_records_safety_pin_dispatch_off_reason() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_SRC, b"=fib28_selflink_on_label")
        .expect("loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");

    let reasons = vm.trace_dispatch_off_reasons();
    // Accept either the SelfLink label OR the DownRec stitch-pending
    // label; both signal the safety pin.
    let safety_pin_label_fired =
        reasons.contains(&"self-link-retf-r1") || reasons.contains(&"downrec-stitch-pending");
    assert!(
        safety_pin_label_fired,
        "expected at least one trace pinned dispatch_off via the SelfLink \
         deopt OR the DownRec stitch-pending fallback — got reasons: {:?}",
        reasons
    );
}
