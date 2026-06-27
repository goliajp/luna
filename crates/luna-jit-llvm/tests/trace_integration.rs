//! v2.1 Phase 1K.G.5 — E2E integration test for the LLVM trace JIT MVP.
//!
//! Builds a synthetic `TraceRecord` containing only MVP-whitelist ops
//! (`LoadI`, `Add`, `Jmp`), calls `LlvmBackend::try_compile_trace`, and
//! verifies:
//! 1. The call returns `Some(CompiledTrace)` with `dispatchable: true`.
//! 2. Invoking the JIT-compiled `TraceFn` with a reg-state buffer returns
//!    the `head_pc` (clean-tail path, since the synthetic trace has no
//!    conditional branches that can side-exit).
//!
//! A second sub-test builds a `TraceRecord` that would be rejected by the
//! MVP whitelist (`GetUpval` op at depth=0) and asserts `None`.
//!
//! The test constructs `TraceRecord` from a real `Gc<Proto>` materialised
//! via `vm.load`; all `RecordedOp`s are injected synthetically so the
//! test doesn't depend on a live recording run.

use luna_core::jit::TraceCompiler;
use luna_core::jit::trace_types::{RecordedOp, TraceRecord};
use luna_core::vm::isa::{Inst, Op};
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

/// Helper: build a synthetic `RecordedOp` at depth=0.
fn recorded(proto: luna_core::runtime::Gc<luna_core::runtime::function::Proto>, pc: u32, inst: Inst) -> RecordedOp {
    RecordedOp {
        proto,
        pc,
        inst,
        inline_depth: 0,
        var_count: None,
    }
}

/// v2.1 Phase 1K.G.5 — MVP trace compiles and runs (clean tail).
///
/// Trace shape (3 ops at head_pc=0, window_size = proto.max_stack):
///   [0] LoadI  R0, 42       — R0 = 42
///   [1] Add    R1, R0, R0   — R1 = R0 + R0 = 84
///   [2] Jmp    sj=0         — back-edge (Jmp with sj=-1 targets pc 2,
///                              effectively a closed-loop sentinel for the test)
///
/// The clean-tail path should return `head_pc = 0`.
/// After the trace runs, reg_state[0] == 42 and reg_state[1] == 84.
#[test]
fn mvp_trace_compiles_and_runs_clean_tail() {
    // Materialise a real Gc<Proto> via the parser. The source is a
    // two-op chunk; we only need a valid proto to satisfy the type.
    // max_stack must be ≥ 2 to fit R0 + R1.
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local a = 1; local b = 2; return a + b", b"=trace_test")
        .expect("parse");
    let proto = closure.proto;

    // Verify the proto has at least 2 stack slots for our synthetic ops.
    assert!(
        proto.max_stack >= 2,
        "need max_stack >= 2, got {}",
        proto.max_stack
    );

    // Build a synthetic closed TraceRecord.
    let mut record = TraceRecord::start(proto, 0, vec![], false);
    // [0] LoadI R0, 42
    record.push(recorded(proto, 0, Inst::iasbx(Op::LoadI, 0, 42)));
    // [1] Add R1, R0, R0
    record.push(recorded(proto, 1, Inst::iabc(Op::Add, 1, 0, 0, false)));
    // [2] Jmp sj=−1 — standard loop back-edge; in a closed trace the
    //     recorder fires exactly at re-entry of head_pc. For the MVP test
    //     the Jmp op causes `compile_trace_fn` to emit `br clean_tail_bb`
    //     and stop.
    record.push(recorded(proto, 2, Inst::isj(Op::Jmp, -1)));
    record.closed = true;

    // Compile via the LLVM trace lowerer.
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let ct = backend
        .try_compile_trace(
            &mut storage,
            &record,
            luna_core::jit::trace_types::CompileOptions::default(),
        )
        .expect("MVP trace should compile to Some(CompiledTrace)");

    assert!(ct.dispatchable, "depth=0 int-only trace must be dispatchable");
    assert_eq!(ct.head_pc, 0, "head_pc round-trip");
    assert_eq!(ct.n_ops, 3, "3 recorded ops");

    // Invoke the JIT-compiled TraceFn.
    //
    // ABI: extern "C" fn(reg_state: *mut i64) -> i64.
    // `reg_state` is a buffer of `max_stack` i64 slots. The trace reads
    // and writes slots 0..=1 and returns `head_pc` (0) on the clean-tail
    // path.
    let max_stack = proto.max_stack as usize;
    let mut reg_state = vec![0i64; max_stack];

    // SAFETY: `ct.entry` was compiled from MVP IR matching the TraceFn
    // ABI. `ct` holds an `EnginePair` (parked in `storage`) that keeps
    // the mcode alive for at least the duration of this test.
    let returned: i64 = unsafe { (ct.entry)(reg_state.as_mut_ptr()) };

    assert_eq!(
        returned, 0,
        "clean-tail path must return head_pc (0), got {returned}"
    );
    assert_eq!(reg_state[0], 42, "R0 should be 42 after LoadI");
    assert_eq!(reg_state[1], 84, "R1 should be 84 after Add(R0, R0)");
}

/// v2.1 Phase 1K.G.5 — out-of-whitelist trace returns None.
///
/// Injects an `Op::GetUpval` op (outside the MVP set) at depth=0.
/// `try_compile_trace` must return `None` without panicking.
#[test]
fn out_of_whitelist_trace_returns_none() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = 1; return x", b"=reject_test")
        .expect("parse");
    let proto = closure.proto;

    let mut record = TraceRecord::start(proto, 0, vec![], false);
    // GetUpval is outside the MVP whitelist.
    record.push(recorded(proto, 0, Inst::iabc(Op::GetUpval, 0, 0, 0, false)));
    record.push(recorded(proto, 1, Inst::isj(Op::Jmp, -1)));
    record.closed = true;

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let result = backend.try_compile_trace(
        &mut storage,
        &record,
        luna_core::jit::trace_types::CompileOptions::default(),
    );

    assert!(result.is_none(), "GetUpval trace must return None from MVP lowerer");
}

/// v2.1 Phase 1K.G.5 — unclosed trace (closed=false) returns None.
#[test]
fn unclosed_trace_returns_none() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = 1; return x", b"=unclosed_test")
        .expect("parse");
    let proto = closure.proto;

    let mut record = TraceRecord::start(proto, 0, vec![], false);
    record.push(recorded(proto, 0, Inst::iasbx(Op::LoadI, 0, 1)));
    // closed remains false (default from TraceRecord::start)

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let result = backend.try_compile_trace(
        &mut storage,
        &record,
        luna_core::jit::trace_types::CompileOptions::default(),
    );

    assert!(result.is_none(), "unclosed trace must return None");
}
