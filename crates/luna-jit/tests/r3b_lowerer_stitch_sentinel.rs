//! v2.0 Track-R R3b — lowerer stitch-sentinel + caller-pc guard
//! regression pin.
//!
//! R3b adds the lowerer side of the LuaJIT `asm_retf` /
//! `asm_tail_link` shape (`lj_asm_arm64.h:565` / `lj_asm.c:2131`)
//! for luna's `TraceEnd::DownRec` close. The lowerer's
//! `downrec_idx_opt` arm at
//! `crates/luna-jit/src/jit_backend/trace.rs:7129+` emits:
//!
//! 1. A guard predicate: `saved_pc == iconst(dr_return_pc)`. Today
//!    `saved_pc` is `iconst(0)` (placeholder — luna's trace ABI
//!    `fn(reg_state) -> i64` has no dedicated saved-PC slot; R3c
//!    must populate one via the dispatcher pre-invoke). cranelift
//!    constant-folds the compare to always-false for valid
//!    recordings (where `dr_return_pc != 0`), so the emitted
//!    machine code is identical to R3a's safe deopt-tail.
//!
//! 2. A stitch path that, on guard hit, returns
//!    `(1<<63) | (SIDE_SENT_DOWNREC_CODE<<56) | head_pc` so the
//!    dispatcher (R3c) decodes through the side-trace marker and
//!    routes via `CompiledTrace.downrec_link`.
//!
//! 3. A deopt path identical to R3a's safe fall-through (store back
//!    caller window + return `head_pc` via the GLOBAL sentinel).
//!
//! R3b populates `CompiledTrace.downrec_link =
//! Some((0, record.head_pc))` and pins `dispatchable = false` (R3d
//! lifts) with `dispatch_off_reason = "downrec-stitch-pending"` so
//! a probe can distinguish R3b's scaffold-stage close from R1's
//! safe deopt.
//!
//! Workload: fib(3) hot loop (per R3a verdict §5). fib(28) p16-on
//! closes via SelfLink BEFORE base-case Returns reach the recorder,
//! so DownRec doesn't fire on it. fib(3) called in a hot loop hits
//! base cases — the 3rd RetfRecord targeting fib's proto trips the
//! threshold and the recorder stamps `downrec_close`; the lowerer's
//! DownRec arm runs.
//!
//! Asserts:
//! - fib(3) hot loop returns the correct sum (200 * 2 = 400).
//! - `vm.trace_downrec_link_compiled_count() >= 1` — at least one
//!   compiled trace carries `downrec_link = Some(_)`.
//! - `close_cause_counts["downrec-stitch-pending"] >= 1` — the
//!   lowerer's R3b dispatch_off label fired, which (by virtue of
//!   how the arm assigns both fields together) implies the same
//!   trace pinned `dispatchable = false`.
//! - `close_cause_counts["downrec-restart"] >= 1` — R3a regression
//!   (recorder fired before lowerer).
//!
//! R3c will add a follow-up test that asserts the dispatcher
//! consumes the DOWNREC sentinel. R3d will lift `dispatchable`
//! and pin `trace_dispatched > 0` on fib(28).

use luna_jit::version::LuaVersion;

/// fib(3) called in a hot loop. fib(3) recurses 5× total, depth ≤ 2
/// → no self-link cycle trip (`RECUNROLL_THRESHOLD = 2` requires
/// count > 2). Base cases (`n < 2 -> return n`) ARE reached during
/// recording, so depth>0 Returns push RetfRecords. 200 outer iters
/// × 5 inner calls = 1000 bumps on fib's `call_hot_count` (threshold
/// 64 → ~13 iters before the recorder fires); recording captures
/// the full fib(3) recursion tree's retfs.
const FIB_3_HOT_LOOP_SRC: &[u8] = b"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    local s = 0
    for i = 1, 200 do s = s + fib(3) end
    return s
";

// fib(3) = 2; 200 iters × 2 = 400.
const EXPECTED_FIB_3_HOT_LOOP_SUM: i64 = 400;

/// R3b primary pin: lowerer's `downrec_idx_opt` arm emits the
/// stitch-sentinel + caller-pc-guard scaffold AND populates
/// `CompiledTrace.downrec_link` on the fib(3) hot-loop shape.
#[test]
fn p16_on_fib_3_hot_loop_compiles_trace_with_downrec_link_some() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm
        .load(FIB_3_HOT_LOOP_SRC, b"=fib3_loop_r3b")
        .expect("fib(3) loop loads");
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("fib(3) loop runs");

    // 1. Result correctness — R3b's IR scaffold must not regress.
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(3) loop returned an unexpected Value: {:?}", other),
    };
    assert_eq!(
        returned, EXPECTED_FIB_3_HOT_LOOP_SUM,
        "R3b regression — fib(3) loop sum must stay {EXPECTED_FIB_3_HOT_LOOP_SUM}"
    );

    // 2. R3a recorder-side: the threshold catch tripped.
    let counts = vm.trace_close_cause_counts();
    let downrec_restart = counts.get("downrec-restart").copied().unwrap_or(0);
    assert!(
        downrec_restart >= 1,
        "R3a regression — fib(3) hot loop must bump downrec-restart \
         >= 1 (got {downrec_restart}; full counts: {counts:?})"
    );

    // 3. R3d post-lift: when the multi-way guard collected >= 2
    //    distinct candidates, the lowerer lifts `dispatchable=true`
    //    and the `"downrec-stitch-pending"` label is NOT pushed.
    //    Either branch is acceptable for the R3b scaffold smoke —
    //    the assertion that "the downrec arm fired" is captured by
    //    `downrec_link_compiled >= 1` below. R3d's
    //    `r3d_multi_way_guard_dispatch.rs` test pins the post-lift
    //    side directly.
    let downrec_stitch_pending = counts.get("downrec-stitch-pending").copied().unwrap_or(0);
    let multi_way = vm.trace_multi_way_guard_emitted_count();
    assert!(
        downrec_stitch_pending + multi_way >= 1,
        "R3b/R3d smoke — the downrec lowerer arm must fire at least \
         once. Either `downrec-stitch-pending` (R3c-shape single-CMP \
         fallback) or `multi_way_guard_emitted` (R3d-shape lifted) \
         must be >= 1. Got pending={downrec_stitch_pending} \
         multi_way={multi_way} (full counts: {counts:?})"
    );

    // 4. R3b's main contract: CompiledTrace.downrec_link populated.
    //    R3d preserves this — the lift only changes dispatchable
    //    + the dispatch_off_reason label, not the downrec_link field.
    let downrec_link_compiled = vm.trace_downrec_link_compiled_count();
    assert!(
        downrec_link_compiled >= 1,
        "R3b expected at least one compiled trace with \
         downrec_link = Some(_), got {downrec_link_compiled}"
    );
}

/// fib(28) p16-on regression: closes via SelfLink BEFORE DownRec
/// can fire, so R3b's downrec_link counter does NOT bump on this
/// shape. Result correctness pin (R1 regression-free).
#[test]
fn p16_on_fib_28_self_link_does_not_bump_downrec_link_compiled() {
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
    vm.set_p16_self_link_enabled(true);
    vm.open_base();

    let cl = vm.load(FIB_28_SRC, b"=fib28_r3b").expect("loads");
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
        "R1 regression — fib(28) must stay {EXPECTED_FIB_28} on p16-on"
    );

    let downrec_link_compiled = vm.trace_downrec_link_compiled_count();
    assert_eq!(
        downrec_link_compiled, 0,
        "fib(28) p16-on closes via SelfLink before DownRec catch \
         can fire — R3b's downrec_link_compiled must NOT bump on \
         this shape. Got {downrec_link_compiled}."
    );
}

/// p16-off gate check: with the cycle catch + recorder DownRec gate
/// off, neither R3a's threshold catch nor R3b's lowerer arm fires.
/// fib(28) still returns correctly (interp-only path).
#[test]
fn p16_off_fib_28_no_downrec_link_compiled() {
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

    let cl = vm.load(FIB_28_SRC, b"=fib28_r3b_p16off").expect("loads");
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
        "fib(28) p16-off must stay {EXPECTED_FIB_28}"
    );

    let downrec_link_compiled = vm.trace_downrec_link_compiled_count();
    assert_eq!(
        downrec_link_compiled, 0,
        "p16-off must not bump downrec_link_compiled — got {downrec_link_compiled}"
    );
}
