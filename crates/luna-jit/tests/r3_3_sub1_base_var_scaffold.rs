//! v2.0 Track-R R3.3+ sub-1 — `base_var` scaffold regression pin.
//!
//! Sub-1 introduces the depth-relative `base_var` Variable at the
//! trace head (post `regs_full` reg-load prelude, before the
//! body_loop jump). The Variable is initialised to `iconst(0)` as
//! the depth-0 sentinel placeholder; sub-2 will replace the init
//! with `reg_state` and start migrating op-arms (Op::Move /
//! Op::LoadK / Op::LoadNil) to load/store via
//! `current_base_addr(bcx, base_var, op_offset_bytes, slot)`.
//!
//! This test pins:
//!
//! 1. **scaffold declared** — `BASE_VAR_SCAFFOLD_DECLARED` bumps by
//!    at least 1 per compiled trace. Exposed via
//!    `luna_jit::jit::trace::base_var_scaffold_declared_count` /
//!    `reset_base_var_scaffold_declared_count`. This proves the
//!    declare_var + def_var(iconst(0)) path actually ran.
//!
//! 2. **fib(28) result correctness preserved** — Row 1/2/3
//!    equivalent fib(28) trace JIT runs return 317811. Sub-1 is
//!    scaffold-only (no op-arm uses base_var yet), so this MUST
//!    stay green; a failure means the scaffold somehow leaked into
//!    op-arm semantics.
//!
//! 3. **sub-0 invariant preserved** — the
//!    `"selflink-yields-to-downrec"` close-cause label still bumps
//!    on fib(28) p16-on, matching the sub-0 regression in
//!    `r3_3_sub0_selflink_relax.rs`. Sub-1's scaffold add must not
//!    perturb the recorder-side close-cause routing.
//!
//! See `.dev/rfcs/v2.0-track-r-r3-3-rfc.md` §6 sub-step 1 for the
//! design contract and `.dev/rfcs/v2.0-track-r-r3-3-sub1-verdict.md`
//! (this commit) for the scaffold shape decision + sub-2 handoff.

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

/// Primary pin: an arbitrary trace JIT compile bumps the
/// `BASE_VAR_SCAFFOLD_DECLARED` counter, proving the sub-1 scaffold
/// ran end-to-end inside `lower_trace_into_named`.
#[test]
fn fib_3_hot_loop_bumps_base_var_scaffold_declared_at_least_once() {
    // Counter is thread-local; reset to 0 so we can pin "this call
    // bumped it" without depending on prior tests in the same thread.
    luna_jit::jit::trace::reset_base_var_scaffold_declared_count();
    assert_eq!(
        luna_jit::jit::trace::base_var_scaffold_declared_count(),
        0,
        "reset_base_var_scaffold_declared_count must restart counter at 0"
    );

    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_hot_sub1_scaffold")
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
        "sub-1 scaffold must not break fib(3) hot loop correctness — \
         expected {EXPECTED_FIB_3_HOT_SUM}"
    );

    let compiled = vm.trace_compiled_count();
    assert!(
        compiled >= 1,
        "fib(3) hot loop must trigger at least one trace compile to \
         exercise the sub-1 scaffold path. Got compiled={compiled}"
    );

    let declared = luna_jit::jit::trace::base_var_scaffold_declared_count();
    assert!(
        declared >= compiled as u64,
        "R3.3+ sub-1 pin — the base_var scaffold's declare_var + \
         iconst(0) init must fire at LEAST once per compiled trace. \
         Got declared={declared}, compiled={compiled}. A count below \
         compiled means the scaffold add at lower_trace_into_named \
         entry block was skipped or short-circuited — sub-2 op-arm \
         migration has no Variable to hang loads/stores on."
    );
}

/// Correctness pin: fib(28) p16-on under the sub-1 scaffold still
/// returns 317_811. Mirrors `r3_3_sub0_selflink_relax.rs`'s primary
/// pin to confirm the scaffold add introduces no off-by-one or
/// stale-frame regression at the trace head.
#[test]
fn p16_on_fib_28_result_correct_under_sub1_scaffold() {
    luna_jit::jit::trace::reset_base_var_scaffold_declared_count();

    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_28_SRC, b"=fib28_sub1_correctness")
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
        "R1 correctness regression — fib(28) p16-on under sub-1 \
         scaffold must stay {EXPECTED_FIB_28}. Sub-1 is scaffold-only \
         (no op-arm uses base_var); a wrong result means the scaffold \
         leaked into op semantics, e.g. via a stale def_var or a \
         duplicated entry-block jump."
    );

    let declared = luna_jit::jit::trace::base_var_scaffold_declared_count();
    assert!(
        declared >= 1,
        "fib(28) p16-on must compile at least one trace and bump the \
         scaffold counter. Got declared={declared}"
    );
}

/// Sub-0 invariant pin: the `"selflink-yields-to-downrec"` close-
/// cause label still bumps on fib(28) p16-on. Sub-1's scaffold add
/// runs in the lowerer; the recorder's SelfLink trip → DownRec relax
/// (sub-0) lives in `crates/luna-core/src/vm/exec.rs` and must NOT
/// regress under the sub-1 changes.
#[test]
fn p16_on_fib_28_preserves_sub0_selflink_yields_to_downrec() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_28_SRC, b"=fib28_sub1_sub0_invariant")
        .expect("fib(28) loads");
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("fib(28) runs");

    let counts = vm.trace_close_cause_counts();
    let yields = counts
        .get("selflink-yields-to-downrec")
        .copied()
        .unwrap_or(0);
    assert!(
        yields >= 1,
        "R3.3+ sub-0 invariant — fib(28) p16-on must still bump \
         \"selflink-yields-to-downrec\" >= 1 under sub-1. Got \
         {yields} (full counts: {counts:?}). A miss here means sub-1 \
         perturbed the recorder's SelfLink trip routing, which is \
         outside the sub-1 scope."
    );
    let r1_label = counts.get("self-link-retf-r1").copied().unwrap_or(0);
    assert_eq!(
        r1_label, 0,
        "R3.3+ sub-0 retired the self-link-retf-r1 path for fib(28); \
         it must stay 0 under sub-1. Got {r1_label} (full counts: \
         {counts:?})."
    );
}
