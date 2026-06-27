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
