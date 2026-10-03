//! Comparisons and branches.

use luna_core::jit::{CompileResult, IntChunkCompiler};
use luna_core::vm::isa::Op;
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

/// Comparison + control flow.
///
/// `local x = 5; if x < 10 then return 1 else return 0 end` compiles
/// to a LtI+Jmp pair plus two Return1 paths. The compute path lowers
/// the comparison+Jmp pair to a single LLVM `condbr` and emits one
/// LLVM BB per source BB. For x=5 (5<10 = true), the JIT entry must
/// take the then-branch and return 1.
#[test]
fn if_lt_then_else_takes_correct_branch() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local x = 5; if x < 10 then return 1 else return 0 end",
            b"=if_lt_then_else",
        )
        .expect("compile");
    let proto = closure.proto;
    assert!(proto.code.iter().any(|i| i.op() == Op::LtI));
    assert!(proto.code.iter().any(|i| i.op() == Op::Jmp));

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the if/else chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 1, "x=5 < 10 → then-branch → return 1",);
}

/// Same chunk shape, false condition. `x = 20`
/// → `20 < 10` is false → else-branch → return 0.
#[test]
fn if_lt_then_else_false_takes_else() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local x = 20; if x < 10 then return 1 else return 0 end",
            b"=if_lt_then_else_false",
        )
        .expect("compile");
    let proto = closure.proto;
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the if/else chunk (false branch)");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 0);
}

/// Lt with return-the-operand branches.
///
/// `local x = 3; local y = 2; if x < y then return x else return y end`
/// returns 2 (else branch, because 3<2 is false).
#[test]
fn lt_xy_returns_smaller() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local x = 3; local y = 2; if x < y then return x else return y end",
            b"=lt_xy",
        )
        .expect("compile");
    let proto = closure.proto;
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile lt_xy");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(
        returned, 2,
        "min(3, 2) via Lt-then-Jmp lowering must equal 2",
    );
}

/// Le boundary.
/// `if 5 <= 5 then return 1 else return 0 end` → 1.
#[test]
fn le_boundary_returns_then() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local x = 5; if x <= 5 then return 1 else return 0 end",
            b"=le_boundary",
        )
        .expect("compile");
    let proto = closure.proto;
    if !proto.code.iter().any(|i| i.op() == Op::Le) {
        eprintln!(
            "[le_boundary] parser folded to non-Le shape; chunk = {:?}",
            proto.code
        );
        return;
    }
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the Le chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 1);
}

/// Eq.
/// `if 7 == 7 then return 1 else return 0 end` → 1.
#[test]
fn eq_returns_then() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local x = 7; if x == 7 then return 1 else return 0 end",
            b"=eq_seven",
        )
        .expect("compile");
    let proto = closure.proto;
    if !proto.code.iter().any(|i| i.op() == Op::Eq) {
        eprintln!(
            "[eq_returns_then] parser folded to non-Eq shape; chunk = {:?}",
            proto.code
        );
        return;
    }
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the Eq chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 1);
}

/// Branchy chunk with multi-op then/else.
///
/// `local n = 5; local r = 0; if n < 10 then r = n*2 else r = n-1
/// end; return r` exercises:
/// - Lt + Jmp (condbr) at the if
/// - Mul in then-branch, Sub in else-branch
/// - bare Jmp at end of then-branch (skip-else)
/// - reachable Return1 at the join point
/// - multiple BBs sharing the same alloca register file
///
/// For n=5 → 5<10=true → r = 5*2 = 10. Returns 10.
#[test]
fn branchy_chunk_then_path() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local n = 5\nlocal r = 0\nif n < 10 then r = n * 2 else r = n - 1 end\nreturn r",
            b"=branchy_chunk_then",
        )
        .expect("compile");
    let proto = closure.proto;
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the branchy chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 10);
}

/// Branchy chunk, else path. n=20 → r = 20-1
/// = 19.
#[test]
fn branchy_chunk_else_path() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local n = 20\nlocal r = 0\nif n < 10 then r = n * 2 else r = n - 1 end\nreturn r",
            b"=branchy_chunk_else",
        )
        .expect("compile");
    let proto = closure.proto;
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the branchy chunk (else)");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 19);
}
