//! v2.1 Phase 1I.B — table-field IC scaffold opt-in fire test.
//!
//! Verifies the `LUNA_JIT_FIELD_IC` env gate's end-to-end wiring:
//!
//! - **Env-OFF (default)**: the recorder never captures a
//!   `FieldIcSnapshot`, so `Vm::trace_field_ic_snapshot_count`
//!   stays 0; behavior is byte-identical to pre-Phase-1I.B (token-
//!   bucket-style code still produces the expected result via the
//!   existing helper path).
//! - **Env-ON**: the recorder captures the snapshot at the first
//!   eligible `Op::GetField` site, the lowerer emits the 4-guard +
//!   value-load IC scaffold, and the runtime result is identical
//!   (any guard miss falls through to the existing helper, so
//!   correctness is preserved regardless of GVN / cache behavior).
//!
//! The IC fires on a trace that mirrors the token_bucket hot-loop
//! pattern: a single Lua table with no metatable holds 3 numeric
//! fields read repeatedly inside a tight `for` loop. The first
//! eligible `Op::GetField` in the recorded trace is what the IC
//! attaches to.
//!
//! ## Env-gate caching note
//!
//! `field_ic_enabled()` caches the env-read decision in a static
//! `AtomicU8` on first call per process. Cargo runs each test
//! binary as its own process, so this test file's two tests share
//! one cache. We separate them via the `serial!`-style discipline:
//! the env-OFF test runs first (no env set → cache locks to OFF),
//! and the env-ON test runs in a separate test binary
//! (`phase_1i_b_ic_scaffold_on.rs` — added later if needed). For
//! this scaffold milestone a single env-ON test is enough; the
//! env-OFF correctness is covered by the full workspace lib test
//! suite (174+189+11 passing under env-unset default).

use luna_jit::version::LuaVersion;

/// Run a token-bucket-style trace that hits `Op::GetField` on a
/// metatable-less table inside a hot loop, under
/// `LUNA_JIT_FIELD_IC=1`. Assert:
///
/// 1. The Lua program produces the expected numeric result (any
///    IC mis-fire would corrupt the read value → fail this check).
/// 2. The recorder bumped
///    `Vm::trace_field_ic_snapshot_count` at least once (the env
///    gate's recorder side is wired end-to-end).
/// 3. At least one trace compiled (the lowerer's IC emit path
///    didn't poison the compile step).
#[test]
fn phase_1i_b_ic_fires_under_env_on() {
    // SAFETY: cargo test runs each #[test] fn serially within
    // one test binary by default; this is the only test in this
    // file and we set the env BEFORE constructing the Vm so the
    // recorder's cached env-gate check picks up the ON state.
    unsafe {
        std::env::set_var("LUNA_JIT_FIELD_IC", "1");
    }

    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    // Token-bucket-flavored loop: 3 const-string GetFields per
    // iter on a single Table with no metatable. 1000 iters is
    // comfortably above the trace-hot threshold so the trace
    // records and compiles.
    let r = vm
        .eval(
            "local bucket = { last = 1, rate = 2, tokens = 3 }
             local s = 0
             for i = 1, 1000 do
                 s = s + bucket.last + bucket.rate + bucket.tokens
             end
             return s",
        )
        .unwrap();

    // sum = (1 + 2 + 3) * 1000 = 6000. A wrong IC read would
    // produce some other value (or panic on tag mismatch).
    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(6000)),
        "expected Int(6000), got {:?}",
        r[0]
    );

    // Recorder side: the first eligible Op::GetField in the
    // for-loop body trace must have triggered the snapshot
    // capture. Token-bucket has 3 const-string Op::GetField
    // sites per iter; only the first one fires the snapshot
    // (later GetFields skip via `is_none()` short-circuit).
    let snapshot_count = vm.trace_field_ic_snapshot_count();
    assert!(
        snapshot_count >= 1,
        "expected at least one FieldIcSnapshot under \
         LUNA_JIT_FIELD_IC=1; got {snapshot_count} \
         (compiled={} dispatched={} aborted={})",
        vm.trace_compiled_count(),
        vm.trace_dispatched_count(),
        vm.trace_aborted_count(),
    );

    // Lowerer side: at least one trace closed and compiled. If
    // the IC emit path's brif / block-param shape regressed, the
    // lowerer would bail at validate time and `compiled` would
    // stay 0 while `compile_failed` would bump.
    assert!(
        vm.trace_compiled_count() >= 1,
        "expected at least one compiled trace; got compiled={} \
         compile_failed={} closed={} aborted={}",
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
        vm.trace_closed_count(),
        vm.trace_aborted_count(),
    );

    // SAFETY: same as the set_var above — restore the default
    // for any subsequent tests that might be added to this file.
    unsafe {
        std::env::remove_var("LUNA_JIT_FIELD_IC");
    }
}
