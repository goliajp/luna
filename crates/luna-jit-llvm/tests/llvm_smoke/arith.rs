//! Integer returns and arithmetic.

use luna_core::jit::{CompileResult, IntChunkCompiler};
use luna_core::vm::isa::Op;
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

/// First observable-value chunk JIT.
///
/// `return 42` compiles to `[LoadI(R0, 42), Return1(R0), Return0]`.
/// The compute path's recognised prefix is `(LoadI|LoadNil|Move)*`
/// terminated by `Return1`. The JIT entry returns the i64 stored in
/// `regs[0]`, which is the immediate `42`.
#[test]
fn return_i_compiles_and_returns_immediate() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(b"return 42", b"=return_42").expect("compile");
    let proto = closure.proto;

    let code: &[_] = &proto.code;
    assert!(
        code.len() >= 2,
        "expected at least LoadI + Return1; got {code:?}"
    );
    assert_eq!(code[0].op(), Op::LoadI);
    assert_eq!(code[0].sbx(), 42);
    assert_eq!(code[1].op(), Op::Return1);

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let result = backend.try_compile(&mut storage, proto, false, false);
    let CompileResult::Compiled {
        entry,
        returns_one,
        ret_is_float,
        ..
    } = result
    else {
        panic!("must compile `return 42`; got {result:?}");
    };
    assert!(
        returns_one,
        "Return1 chunk must report returns_one=true (got {returns_one})",
    );
    assert!(!ret_is_float, "int-immediate chunk returns i64, not f64");
    assert!(!entry.is_null(), "compute path must yield a non-null entry");

    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(
        returned, 42,
        "Return1 chunk's JIT entry must return the loaded immediate"
    );
}

/// Negative immediate path. `return -7` lowers
/// to `[LoadI(R0, -7), Return1(R0)]`; verify the sign-extension is
/// preserved through the IR `const_int(i64, signed=true)` cast.
#[test]
fn return_i_handles_negative_immediate() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(b"return -7", b"=return_neg7").expect("compile");
    let proto = closure.proto;
    // The parser may fold `-7` to a LoadI of -7, or to a LoadI of 7
    // + a unary `Unm`. Skip if it's the latter — the compute path
    // doesn't cover Unm.
    if !matches!(proto.code.first().map(|i| i.op()), Some(Op::LoadI))
        || proto.code.first().map(|i| i.sbx()) != Some(-7)
    {
        // Parser folded differently; the negative-immediate smoke is
        // not exercised by this source on this dialect. Bail rather
        // than assert against a parser shape we don't control.
        return;
    }

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("`return -7` must compile");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, -7);
}

/// Int Add smoke.
///
/// `local x = 2; local y = 3; return x + y` compiles to
/// `[LoadI(R0,2), LoadI(R1,3), Add(R2,R0,R1), Return1(R2), Return0]`.
/// The compute path's Add emit lowers to `regs[2] = regs[0] +
/// regs[1]` (i64 add), and the Return1 reads back the i64 to deliver
/// `5` through the dispatcher contract.
#[test]
fn add_two_loaded_ints() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = 2; local y = 3; return x + y", b"=add_xy")
        .expect("compile");
    let proto = closure.proto;

    // Confirm the parser shape — if it ever folds the `2 + 3` at
    // parse time into `return 5`, this test guards the change.
    let code: &[_] = &proto.code;
    assert!(
        code.iter().any(|i| i.op() == Op::Add),
        "test source must emit an Add to exercise \
         (got {code:?})",
    );

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile `local x=2; local y=3; return x+y`");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 5, "2 + 3 must equal 5 through the JIT entry");
}

/// Add with negative + positive operands. Pins
/// signed-i64 semantics (no wrap surprise for small inputs).
#[test]
fn add_negative_and_positive() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = -10; local y = 4; return x + y", b"=add_neg_pos")
        .expect("compile");
    let proto = closure.proto;
    // Parser may fold -10 or use Unm; require LoadI(-10) prefix to
    // stay inside the compute whitelist.
    let has_neg_loadi = proto
        .code
        .iter()
        .any(|i| i.op() == Op::LoadI && i.sbx() == -10);
    let has_add = proto.code.iter().any(|i| i.op() == Op::Add);
    if !has_neg_loadi || !has_add {
        eprintln!(
            "[add_negative_and_positive] parser shape differs from \
             target; chunk = {:?}",
            proto.code
        );
        return;
    }

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the neg+pos add chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, -6);
}

/// Int Sub / Mul smoke.
#[test]
fn sub_two_loaded_ints() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = 7; local y = 5; return x - y", b"=sub_xy")
        .expect("compile");
    let proto = closure.proto;
    assert!(proto.code.iter().any(|i| i.op() == Op::Sub));

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the Sub chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 2);
}

#[test]
fn mul_two_loaded_ints() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = 6; local y = 7; return x * y", b"=mul_xy")
        .expect("compile");
    let proto = closure.proto;
    assert!(proto.code.iter().any(|i| i.op() == Op::Mul));

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the Mul chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 42);
}

/// Lua-semantic Mod, positive operands.
/// Lua: `17 % 5 == 2`. Matches C srem too.
#[test]
fn mod_positive_operands() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = 17; local y = 5; return x % y", b"=mod_pos")
        .expect("compile");
    let proto = closure.proto;
    assert!(proto.code.iter().any(|i| i.op() == Op::Mod));

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the Mod chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 2);
}

/// Lua Mod with cross-sign operands. This is the
/// case where Lua's floor-mod differs from C's srem. Lua: `(-7) % 3
/// == 2`, C srem(-7, 3) == -1. The JIT emit's sign-fixup must apply.
///
/// Parser-fold-tolerant: if `-7` is parsed as `LoadI(7) + Unm`
/// (Unm not in the whitelist) the chunk falls outside the
/// recognised shape and the test skips. luna 5.5 emits a direct
/// LoadI(-7).
#[test]
fn mod_negative_dividend_lua_semantics() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = -7; local y = 3; return x % y", b"=mod_neg_div")
        .expect("compile");
    let proto = closure.proto;
    let has_neg_loadi = proto
        .code
        .iter()
        .any(|i| i.op() == Op::LoadI && i.sbx() == -7);
    let has_mod = proto.code.iter().any(|i| i.op() == Op::Mod);
    if !has_neg_loadi || !has_mod {
        eprintln!(
            "[mod_negative_dividend] parser shape differs from \
             target; chunk = {:?}",
            proto.code
        );
        return;
    }

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("must compile the neg-dividend Mod chunk");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(
        returned, 2,
        "Lua `(-7) %% 3` must equal 2 (floor-mod, sign of divisor); \
         got {returned} (C srem would be -1)",
    );
}

/// Div is deliberately NOT in the compute
/// whitelist (Lua semantics returns float). Confirm the chunk bails
/// to interpreter rather than being mis-compiled as int sdiv.
#[test]
fn div_chunk_bails_until_float_support() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = 8; local y = 3; return x / y", b"=div_xy")
        .expect("compile");
    let proto = closure.proto;
    assert!(
        proto.code.iter().any(|i| i.op() == Op::Div),
        "test source must emit a Div op (got {:?})",
        proto.code
    );

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let result = backend.try_compile(&mut storage, proto, false, false);
    assert!(
        matches!(result, CompileResult::Skipped),
        "Op::Div must bail until float support lands (got {result:?})",
    );
}
