//! v2.1 Phase 1K.D.6 — end-to-end smoke for the LLVM backend's
//! single recognised chunk shape (`[Op::LoadNil(_, _), Op::Return0]`).
//!
//! The test:
//! 1. Spins up a `luna_jit` Vm with the default Cranelift backend (we
//!    only need its parser + heap to materialise a `Proto`; the JIT
//!    backend choice is irrelevant — we call `LlvmBackend::try_compile`
//!    directly, bypassing the dispatcher).
//! 2. Loads the Lua source `local x` which compiles to a Proto whose
//!    body is exactly `[LoadNil(R0, 0), Return0]`.
//! 3. Calls `LlvmBackend::try_compile` on that Proto.
//! 4. Asserts the result is `CompileResult::Compiled` with the
//!    expected metadata (zero args, no return value, no float / table
//!    masks).
//! 5. Transmutes the returned entry pointer to
//!    `unsafe extern "C" fn() -> i64` and invokes it; asserts the
//!    call returns 0 (the chunk has no observable return value).
//!
//! This proves the inkwell → LLVM 18 toolchain emits + JIT-compiles +
//! resolves a Rust-callable function pointer through luna's trait
//! surface, with the leaked Context / ExecutionEngine keeping the
//! mmap alive for the duration of the test. Phase 1K.D.7 extends to
//! `Op::LoadK + Op::Move + Op::LoadNil`; Phase 1K.D.8 swaps the leak
//! for a per-`Vm` storage cache.

use luna_core::jit::{CompileResult, IntChunkCompiler};
use luna_core::vm::isa::Op;
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

#[test]
fn load_nil_then_return0_compiles_and_runs() {
    // Build a Vm whose parser can materialise the test Proto. The
    // Vm's backend choice is irrelevant here — `try_compile` runs
    // outside the dispatch path.
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(b"local x", b"=load_nil_smoke").expect("compile");
    let proto = closure.proto;

    // Confirm the parser emitted the expected bytecode shape — if
    // the parser ever changes to fold `local x` to a different
    // chunk, this test guards the change.
    let code: &[_] = &proto.code;
    assert_eq!(
        code.len(),
        2,
        "expected `local x` to compile to 2 ops, got {} ({:?})",
        code.len(),
        code,
    );
    assert_eq!(code[0].op(), Op::LoadNil, "first op should be LoadNil");
    assert_eq!(code[1].op(), Op::Return0, "second op should be Return0");

    // Drive the LLVM backend directly with a fresh storage.
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage;
    let result = backend.try_compile(&mut storage, proto, false, false);

    let (entry, num_args, returns_one, arg_float_mask, arg_table_mask, ret_is_float, ret_is_table) =
        match result {
            CompileResult::Compiled {
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            } => (
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            ),
            CompileResult::Skipped => {
                panic!("LlvmBackend::try_compile returned Skipped for the LoadNil chunk");
            }
        };

    assert_eq!(num_args, 0, "LoadNil chunk has zero JIT-entry args");
    assert!(!returns_one, "LoadNil chunk's Return0 produces no value");
    assert_eq!(arg_float_mask, 0);
    assert_eq!(arg_table_mask, 0);
    assert!(!ret_is_float);
    assert!(!ret_is_table);
    assert!(!entry.is_null(), "LLVM should produce a non-null entry");

    // Invoke the JIT-compiled entry. The return-shape contract is
    // `unsafe extern "C" fn() -> i64`; a Return0 chunk returns 0
    // (the dispatcher knows by `returns_one == false` to ignore it).
    //
    // SAFETY: `entry` was just produced by inkwell's ExecutionEngine
    // and the engine is `Box::leak`-pinned for the process lifetime
    // (see `codegen::compile_constant_zero_chunk`). The IR declared
    // exactly `fn() -> i64` so the C-ABI cast is sound.
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(
        returned, 0,
        "Return0 chunk's JIT entry should return 0 (no value)",
    );
}

/// v2.1 Phase 1K.D.7 — 3-op chunk smoke.
///
/// `local x; local y = 'h'; local z = y` compiles to
/// `[LoadNil(R0,0), LoadK(R1, "h"), Move(R2,R1), Return0]`. The
/// chunk's locals are dead at the `Return0` boundary, so the
/// recognised shape `[(LoadNil | LoadK | Move)*, Return0]` lowers
/// to the same `extern "C" fn() -> i64 { ret 0 }` entry as 1K.D.6's
/// LoadNil-only chunk. Phase 1K.E grows out to ops with observable
/// side effects.
#[test]
fn three_op_dead_locals_chunk_compiles_and_runs() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x; local y = 'h'; local z = y", b"=three_op_smoke")
        .expect("compile");
    let proto = closure.proto;

    let code: &[_] = &proto.code;
    assert_eq!(
        code.len(),
        4,
        "expected 3 work ops + Return0; got {} ({:?})",
        code.len(),
        code,
    );
    assert_eq!(code[0].op(), Op::LoadNil);
    assert_eq!(code[1].op(), Op::LoadK);
    assert_eq!(code[2].op(), Op::Move);
    assert_eq!(code[3].op(), Op::Return0);

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage;
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("LlvmBackend::try_compile returned Skipped for the 3-op chunk");
    };
    assert!(!entry.is_null());

    // SAFETY: matches the 1K.D.6 smoke's calling convention.
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(returned, 0);
}

/// Phase 1K.D.7 — sanity check that out-of-shape chunks bail back
/// to the interpreter rather than being mis-compiled. `return 1`
/// emits `LoadI` + `Return1`, neither of which is in the recognised
/// prefix set; the backend must report `Skipped`.
#[test]
fn out_of_shape_chunk_returns_skipped() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(b"return 1", b"=out_of_shape").expect("compile");
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage;
    let result = backend.try_compile(&mut storage, closure.proto, false, false);
    assert!(
        matches!(result, CompileResult::Skipped),
        "out-of-whitelist chunk must bail (got {result:?})",
    );
}
