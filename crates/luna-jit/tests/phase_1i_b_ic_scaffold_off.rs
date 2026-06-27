//! v2.1 Phase 1I.B — env-OFF default sanity check.
//!
//! Companion to `phase_1i_b_ic_scaffold.rs` (env-ON fire test):
//! verifies the default-OFF path is byte-equivalent to pre-Phase-1I.B
//! semantics. Lives in its own test binary so the `field_ic_enabled()`
//! atomic cache locks to OFF without contention from the env-ON
//! sibling test (cargo gives each `tests/*.rs` its own process).

use luna_jit::version::LuaVersion;

/// Run the same token-bucket-style trace without setting the env
/// var. Assert:
///
/// 1. The Lua program produces the expected result.
/// 2. No `FieldIcSnapshot` is captured
///    (`Vm::trace_field_ic_snapshot_count() == 0`).
/// 3. At least one trace compiled (the env-OFF default path
///    didn't regress the existing helper-call shape).
#[test]
fn phase_1i_b_ic_does_not_fire_under_env_off() {
    // Explicitly clear, in case some other env-source set it for
    // this process (e.g., shell export). SAFETY: cargo test runs
    // each #[test] fn serially within one test binary by default;
    // this is the only test in this file.
    unsafe {
        std::env::remove_var("LUNA_JIT_FIELD_IC");
    }

    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

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

    assert!(
        matches!(r[0], luna_jit::runtime::Value::Int(6000)),
        "expected Int(6000), got {:?}",
        r[0]
    );

    let snapshot_count = vm.trace_field_ic_snapshot_count();
    assert_eq!(
        snapshot_count, 0,
        "env-OFF default must not capture any FieldIcSnapshot; \
         got {snapshot_count}",
    );

    assert!(
        vm.trace_compiled_count() >= 1,
        "env-OFF default path must still compile traces; got \
         compiled={} compile_failed={}",
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
    );
}
