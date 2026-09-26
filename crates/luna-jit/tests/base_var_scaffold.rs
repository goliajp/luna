//! `base_var` scaffold regression pin.
//!
//! The lowerer declares the depth-relative `base_var` Variable at the
//! trace head (post `regs_full` reg-load prelude, before the
//! body_loop jump). The Variable is initialised to `iconst(0)` as
//! the depth-0 sentinel placeholder; no op-arm reads it yet.
//!
//! This test pins:
//!
//! 1. **scaffold declared** — `BASE_VAR_SCAFFOLD_DECLARED` bumps by
//!    at least 1 per compiled trace. Exposed via
//!    `luna_jit::jit::trace::base_var_scaffold_declared_count` /
//!    `reset_base_var_scaffold_declared_count`. This proves the
//!    declare_var + def_var(iconst(0)) path actually ran.
//!
//! 2. **fib(28) result correctness preserved** — fib(28) trace JIT
//!    runs return 317811. The scaffold
//!    is unused by op-arms, so this MUST
//!    stay green; a failure means the scaffold somehow leaked into
//!    op-arm semantics.
//!
//! 3. **SelfLink relax invariant preserved** — the
//!    `"selflink-yields-to-downrec"` close-cause label still bumps
//!    on fib(28) self-link on, matching the regression in
//!    `selflink_yields_to_downrec.rs`. The scaffold must not
//!    perturb the recorder-side close-cause routing.

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
/// `BASE_VAR_SCAFFOLD_DECLARED` counter, proving the scaffold
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
        "base_var scaffold must not break fib(3) hot loop correctness — \
         expected {EXPECTED_FIB_3_HOT_SUM}"
    );

    let compiled = vm.trace_compiled_count();
    assert!(
        compiled >= 1,
        "fib(3) hot loop must trigger at least one trace compile to \
         exercise the base_var scaffold path. Got compiled={compiled}"
    );

    let declared = luna_jit::jit::trace::base_var_scaffold_declared_count();
    assert!(
        declared >= compiled as u64,
        "base_var scaffold pin — the base_var scaffold's declare_var + \
         iconst(0) init must fire at LEAST once per compiled trace. \
         Got declared={declared}, compiled={compiled}. A count below \
         compiled means the scaffold add at lower_trace_into_named \
         entry block was skipped or short-circuited — later op-arm \
         migration has no Variable to hang loads/stores on."
    );
}

/// Correctness pin: fib(28) self-link on with the scaffold still
/// returns 317_811. Mirrors `selflink_yields_to_downrec.rs`'s primary
/// pin to confirm the scaffold add introduces no off-by-one or
/// stale-frame regression at the trace head.
#[test]
fn self_link_on_fib_28_result_correct_under_base_var_scaffold() {
    luna_jit::jit::trace::reset_base_var_scaffold_declared_count();

    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
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
        "correctness regression — fib(28) self-link on under the base_var \
         scaffold must stay {EXPECTED_FIB_28}. The scaffold is declare-only \
         (no op-arm uses base_var); a wrong result means the scaffold \
         leaked into op semantics, e.g. via a stale def_var or a \
         duplicated entry-block jump."
    );

    let declared = luna_jit::jit::trace::base_var_scaffold_declared_count();
    assert!(
        declared >= 1,
        "fib(28) self-link on must compile at least one trace and bump the \
         scaffold counter. Got declared={declared}"
    );
}

/// SelfLink relax invariant pin: the `"selflink-yields-to-downrec"`
/// close-cause label still bumps on fib(28) self-link on. The scaffold
/// runs in the lowerer; the recorder's SelfLink trip → DownRec relax
/// lives in `crates/luna-core/src/vm/exec.rs` and must NOT regress.
#[test]
fn self_link_on_fib_28_preserves_selflink_yields_to_downrec() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_self_link_enabled(true);
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
        "SelfLink-to-DownRec invariant — fib(28) self-link on must still bump \
         \"selflink-yields-to-downrec\" >= 1 under the scaffold. Got \
         {yields} (full counts: {counts:?}). A miss here means the scaffold \
         perturbed the recorder's SelfLink trip routing, which is \
         outside its scope."
    );
    let retf_label = counts.get("self-link-retf-r1").copied().unwrap_or(0);
    assert_eq!(
        retf_label, 0,
        "SelfLink-to-DownRec routing retired the self-link-retf-r1 path for fib(28); \
         it must stay 0 under the scaffold. Got {retf_label} (full counts: \
         {counts:?})."
    );
}
