//! A value moved to the return register, and a function whose only loop
//! jumps to itself.

use luna_core::jit::{CompileResult, IntChunkCompiler};
use luna_core::vm::isa::Op;
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

/// `Move`-through-the-return-slot smoke.
///
/// `local x = 9; return x` compiles to `[LoadI(R0,9), Return1(R0)]`
/// in luna's 5.4/5.5 parser (the return reads the local in place,
/// no Move needed). But `local x = 9; local y = x; return y` exercises
/// the Move emit — `[LoadI(R0,9), Move(R1,R0), Return1(R1)]`.
#[test]
fn move_then_return_propagates_through_reg() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = 9; local y = x; return y", b"=move_then_return")
        .expect("compile");
    let proto = closure.proto;
    // Confirm the Move path is exercised; if the parser folds it away
    // skip rather than assert.
    let has_move = proto.code.iter().any(|i| i.op() == Op::Move);
    if !has_move {
        eprintln!(
            "[move_then_return smoke] parser folded out the Move; \
             chunk = {:?}",
            proto.code
        );
        return;
    }

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("`local x = 9; local y = x; return y` must compile");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(
        returned, 9,
        "Move must propagate the value to the return reg"
    );
}

/// A function whose only loop is a jump to itself stays in the
/// interpreter: compiled, it would spin in native code where the
/// instruction budget and hooks never run.
#[test]
fn jump_to_itself_is_not_compiled() {
    for src in ["while true do end", "::top:: goto top"] {
        let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
        let closure = vm.load(src.as_bytes(), b"=self_jump").expect("compile");
        let proto = closure.proto;
        assert!(
            proto.code.iter().any(|i| i.op() == Op::Jmp && i.sj() == -1),
            "{src}: expected a JMP -1, got {:?}",
            proto.code
        );
        let mut storage = LlvmJitStorage::default();
        let result = LlvmBackend.try_compile(&mut storage, proto, false, false);
        assert!(
            matches!(result, CompileResult::Skipped),
            "{src}: a jump to itself was compiled"
        );
    }
}
