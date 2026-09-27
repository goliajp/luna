//! Lowerer stitch-sentinel + caller-pc guard regression pin.
//!
//! The lowerer implements the LuaJIT `asm_retf` / `asm_tail_link`
//! shape (`lj_asm_arm64.h:565` / `lj_asm.c:2131`) for luna's
//! `TraceEnd::DownRec` close. Its `downrec_idx_opt` arm in
//! `crates/luna-jit/src/jit_backend/trace.rs` emits:
//!
//! 1. A caller-pc guard chain: `saved_pc == iconst(candidate_pc)`
//!    per distinct caller pc, with `saved_pc` read from the extra
//!    `reg_state` slot the dispatcher fills before invoking the
//!    trace.
//!
//! 2. A stitch path that, on guard hit, returns
//!    `(1<<63) | (SIDE_SENT_DOWNREC_CODE<<56) | head_pc` so the
//!    dispatcher decodes through the side-trace marker and routes
//!    via `CompiledTrace.downrec_link`.
//!
//! 3. A deopt path (store back caller window + return `head_pc` via
//!    the GLOBAL sentinel).
//!
//! The lowerer populates `CompiledTrace.downrec_link =
//! Some((0, record.head_pc))`. With a single candidate it keeps
//! `dispatchable = false` and sets `dispatch_off_reason =
//! "downrec-stitch-pending"`; with two or more it lifts
//! `dispatchable = true` (pinned by `downrec_multi_way_guard.rs`).
//!
//! Workload: fib(3) hot loop. fib(3) called in a hot loop hits base
//! cases — the 3rd RetfRecord targeting fib's proto trips the
//! threshold and the recorder stamps `downrec_close`; the lowerer's
//! DownRec arm runs.
//!
//! Asserts:
//! - fib(3) hot loop returns the correct sum (200 * 2 = 400).
//! - `vm.trace_downrec_link_compiled_count() >= 1` — at least one
//!   compiled trace carries `downrec_link = Some(_)`.
//! - `close_cause_counts["downrec-stitch-pending"]` or
//!   `multi_way_guard_emitted` >= 1 — the lowerer's DownRec arm fired.
//! - `close_cause_counts["downrec-restart"] >= 1` — the recorder's
//!   threshold catch fired before the lowerer.

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

/// Primary pin: the lowerer's `downrec_idx_opt` arm emits the
/// stitch-sentinel + caller-pc guard AND populates
/// `CompiledTrace.downrec_link` on the fib(3) hot-loop shape.
#[test]
fn self_link_on_fib_3_hot_loop_compiles_trace_with_downrec_link_some() {
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

    // 1. Result correctness — the downrec IR must not change the result.
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        other => panic!("fib(3) loop returned an unexpected Value: {:?}", other),
    };
    assert_eq!(
        returned, EXPECTED_FIB_3_HOT_LOOP_SUM,
        "DownRec lowerer regression — fib(3) loop sum must stay {EXPECTED_FIB_3_HOT_LOOP_SUM}"
    );

    // 2. Recorder side: the threshold catch tripped.
    let counts = vm.trace_close_cause_counts();
    let downrec_restart = counts.get("downrec-restart").copied().unwrap_or(0);
    assert!(
        downrec_restart >= 1,
        "DownRec recorder regression — fib(3) hot loop must bump downrec-restart \
         >= 1 (got {downrec_restart}; full counts: {counts:?})"
    );

    // 3. When the multi-way guard collected >= 2 distinct
    //    candidates, the lowerer lifts `dispatchable=true` and the
    //    `"downrec-stitch-pending"` label is NOT pushed. Either
    //    branch is acceptable here — the assertion that "the downrec
    //    arm fired" is captured by `downrec_link_compiled >= 1`
    //    below. `downrec_multi_way_guard.rs` pins the lifted
    //    side directly.
    let downrec_stitch_pending = counts.get("downrec-stitch-pending").copied().unwrap_or(0);
    let multi_way = vm.trace_multi_way_guard_emitted_count();
    assert!(
        downrec_stitch_pending + multi_way >= 1,
        "DownRec lowerer smoke — the downrec lowerer arm must fire at least \
         once. Either `downrec-stitch-pending` (single-CMP \
         fallback) or `multi_way_guard_emitted` (multi-way lifted) \
         must be >= 1. Got pending={downrec_stitch_pending} \
         multi_way={multi_way} (full counts: {counts:?})"
    );

    // 4. Main contract: CompiledTrace.downrec_link populated. The
    //    multi-way lift only changes dispatchable + the
    //    dispatch_off_reason label, not the downrec_link field.
    let downrec_link_compiled = vm.trace_downrec_link_compiled_count();
    assert!(
        downrec_link_compiled >= 1,
        "expected at least one compiled trace with \
         downrec_link = Some(_), got {downrec_link_compiled}"
    );
}

/// fib(28) self-link on: the SelfLink trip at `cur_depth >= 2` yields to
/// `downrec_close`, so the trace routes through the lowerer's
/// DownRec arm, which emits the stitch sentinel + caller-pc guard —
/// `downrec_link_compiled` bumps >= 1. The single-candidate guard
/// chain keeps `dispatchable=false`. The result (317_811) must stay
/// correct.
#[test]
fn self_link_on_fib_28_selflink_yields_bumps_downrec_link_compiled() {
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
    vm.set_self_link_enabled(true);
    vm.open_base();

    let cl = vm.load(FIB_28_SRC, b"=fib28").expect("loads");
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
        "regression — fib(28) must stay {EXPECTED_FIB_28} on self-link on"
    );

    let downrec_link_compiled = vm.trace_downrec_link_compiled_count();
    assert!(
        downrec_link_compiled >= 1,
        "SelfLink-to-DownRec pin — fib(28) self-link on now routes the SelfLink \
         trip through the DownRec lowerer arm (selflink-yields lift); \
         downrec_link_compiled must bump >= 1. Got \
         {downrec_link_compiled}."
    );
}

/// self-link off gate check: with the cycle catch + recorder DownRec gate
/// off, neither the recorder's threshold catch nor the lowerer's
/// DownRec arm fires.
/// fib(28) still returns correctly (interp-only path).
#[test]
fn self_link_off_fib_28_no_downrec_link_compiled() {
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
    assert_eq!(
        returned, EXPECTED_FIB_28,
        "fib(28) self-link off must stay {EXPECTED_FIB_28}"
    );

    let downrec_link_compiled = vm.trace_downrec_link_compiled_count();
    assert_eq!(
        downrec_link_compiled, 0,
        "self-link off must not bump downrec_link_compiled — got {downrec_link_compiled}"
    );
}
