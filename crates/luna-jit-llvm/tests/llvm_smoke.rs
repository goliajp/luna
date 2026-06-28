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
    let mut storage = LlvmJitStorage::default();
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
    let mut storage = LlvmJitStorage::default();
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

/// v2.1 Phase 1K.D.8 — verify the per-`Vm` `LlvmJitStorage` cache
/// serves a second compile of the same Proto from cache rather than
/// re-emitting LLVM IR.
///
/// The cache_entry_count probe goes from 0 → 1 after the first
/// compile and stays at 1 after the second. Both compiles return
/// the same entry pointer.
#[test]
fn storage_cache_reuses_compiled_entry() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(b"local x", b"=cache_reuse").expect("compile");
    let proto = closure.proto;

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    assert_eq!(storage.cache_entry_count(), 0);

    let first = backend.try_compile(&mut storage, proto, false, false);
    let CompileResult::Compiled { entry: e1, .. } = first else {
        panic!("first compile must succeed");
    };
    assert_eq!(storage.cache_entry_count(), 1);

    let second = backend.try_compile(&mut storage, proto, false, false);
    let CompileResult::Compiled { entry: e2, .. } = second else {
        panic!("second compile must hit cache and succeed");
    };
    assert_eq!(
        storage.cache_entry_count(),
        1,
        "cache hit must NOT grow cache_entry_count",
    );
    assert!(
        std::ptr::eq(e1, e2),
        "cache hit must return the SAME entry pointer ({e1:?} vs {e2:?})",
    );
}

/// Phase 1K.D.7 / Phase 1K.E.2 — sanity check that out-of-shape
/// chunks bail back to the interpreter rather than being mis-compiled.
///
/// At Phase 1K.D the recognised shape was the dead-locals path only,
/// so `return 1` (LoadI + Return1) bailed. Phase 1K.E.2 added the
/// compute path which now handles `LoadI` + `Return1` — so we need a
/// chunk whose op set falls outside *both* paths to retain
/// `Skipped` coverage. `local t = {}` emits `NewTable` + `Return0`;
/// `NewTable` is in neither whitelist, so the backend bails.
#[test]
fn out_of_shape_chunk_returns_skipped() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(b"local t = {}", b"=out_of_shape").expect("compile");
    let proto = closure.proto;
    // Sanity: confirm the parser still emits NewTable here. If it
    // ever folds to something the whitelists do recognise, swap the
    // test source to a chunk that exercises a confirmed out-of-shape
    // op (e.g. `local s = #t` → Len, or `local _ = io.write` → GetUpval).
    let code: &[_] = &proto.code;
    assert!(
        code.iter().any(|i| i.op() == Op::NewTable),
        "test source must emit a NewTable to keep the out-of-shape gate \
         meaningful (got {code:?})",
    );
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let result = backend.try_compile(&mut storage, proto, false, false);
    assert!(
        matches!(result, CompileResult::Skipped),
        "out-of-whitelist chunk must bail (got {result:?})",
    );
}

/// v2.1 Phase 1K.E.2 — first observable-value chunk JIT.
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
        panic!("Phase 1K.E.2 must compile `return 42`; got {result:?}");
    };
    assert!(
        returns_one,
        "Return1 chunk must report returns_one=true (got {returns_one})",
    );
    assert!(!ret_is_float, "int-immediate chunk returns i64, not f64");
    assert!(!entry.is_null(), "compute path must yield a non-null entry");

    // SAFETY: matches the 1K.D smoke's calling convention; the
    // compute path emits the same `fn() -> i64` shape.
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(
        returned, 42,
        "Return1 chunk's JIT entry must return the loaded immediate"
    );
}

/// v2.1 Phase 1K.E.2 — negative immediate path. `return -7` lowers
/// to `[LoadI(R0, -7), Return1(R0)]`; verify the sign-extension is
/// preserved through the IR `const_int(i64, signed=true)` cast.
#[test]
fn return_i_handles_negative_immediate() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(b"return -7", b"=return_neg7").expect("compile");
    let proto = closure.proto;
    // The parser may fold `-7` to a LoadI of -7, or to a LoadI of 7
    // + a unary `Unm` (Phase 1K.E later sub-phase). Skip if it's the
    // latter — Phase 1K.E.2 doesn't cover Unm.
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
        panic!("`return -7` must compile via Phase 1K.E.2");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(returned, -7);
}

/// v2.1 Phase 1K.E.3 — int Add smoke.
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
        "test source must emit an Add to exercise Phase 1K.E.3 \
         (got {code:?})",
    );

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("Phase 1K.E.3 must compile `local x=2; local y=3; return x+y`");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(returned, 5, "2 + 3 must equal 5 through the JIT entry");
}

/// v2.1 Phase 1K.E.3 — Add with negative + positive operands. Pins
/// signed-i64 semantics (no wrap surprise for small inputs).
#[test]
fn add_negative_and_positive() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local x = -10; local y = 4; return x + y", b"=add_neg_pos")
        .expect("compile");
    let proto = closure.proto;
    // Parser may fold -10 or use Unm; require LoadI(-10) prefix to
    // stay in Phase 1K.E.3 scope.
    let has_neg_loadi = proto
        .code
        .iter()
        .any(|i| i.op() == Op::LoadI && i.sbx() == -10);
    let has_add = proto.code.iter().any(|i| i.op() == Op::Add);
    if !has_neg_loadi || !has_add {
        eprintln!(
            "[add_negative_and_positive] parser shape differs from \
             Phase 1K.E.3 target; chunk = {:?}",
            proto.code
        );
        return;
    }

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("Phase 1K.E.3 must compile the neg+pos add chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(returned, -6);
}

/// v2.1 Phase 1K.E.4 — int Sub / Mul smoke.
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
        panic!("Phase 1K.E.4 must compile the Sub chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 2);
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
        panic!("Phase 1K.E.4 must compile the Mul chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 42);
}

/// v2.1 Phase 1K.E.4 — Lua-semantic Mod, positive operands.
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
        panic!("Phase 1K.E.4 must compile the Mod chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 2);
}

/// v2.1 Phase 1K.E.4 — Lua Mod with cross-sign operands. This is the
/// case where Lua's floor-mod differs from C's srem. Lua: `(-7) % 3
/// == 2`, C srem(-7, 3) == -1. The JIT emit's sign-fixup must apply.
///
/// Parser-fold-tolerant: if `-7` is parsed as `LoadI(7) + Unm`
/// (Unm not in the 1K.E.4 whitelist) the chunk falls outside the
/// recognised shape and the test skips. The bytecode probe at
/// Phase 1K.E.1 confirmed luna 5.5 emits a direct LoadI(-7).
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
             Phase 1K.E.4 target; chunk = {:?}",
            proto.code
        );
        return;
    }

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("Phase 1K.E.4 must compile the neg-dividend Mod chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(
        returned, 2,
        "Lua `(-7) %% 3` must equal 2 (floor-mod, sign of divisor); \
         got {returned} (C srem would be -1)",
    );
}

/// v2.1 Phase 1K.E.4 — Div is deliberately NOT in the compute
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

/// v2.1 Phase 1K.E.5+6 — comparison + control flow.
///
/// `local x = 5; if x < 10 then return 1 else return 0 end` compiles
/// to a Lt+Jmp pair plus two Return1 paths. The compute path lowers
/// the Lt+Jmp pair to a single LLVM `condbr` and emits one LLVM BB
/// per source BB. For x=5 (5<10 = true), the JIT entry must take the
/// then-branch and return 1.
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
    assert!(proto.code.iter().any(|i| i.op() == Op::Lt));
    assert!(proto.code.iter().any(|i| i.op() == Op::Jmp));

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("Phase 1K.E.5+6 must compile the if/else chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(
        unsafe { entry_fn() },
        1,
        "x=5 < 10 → then-branch → return 1",
    );
}

/// v2.1 Phase 1K.E.5+6 — same chunk shape, false condition. `x = 20`
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
        panic!("Phase 1K.E.5+6 must compile the if/else chunk (false branch)");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 0);
}

/// v2.1 Phase 1K.E.5+6 — Lt with return-the-operand branches.
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
        panic!("Phase 1K.E.5+6 must compile lt_xy");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(
        returned, 2,
        "min(3, 2) via Lt-then-Jmp lowering must equal 2",
    );
}

/// v2.1 Phase 1K.E.5+6 — Le boundary.
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
        panic!("Phase 1K.E.5+6 must compile the Le chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 1);
}

/// v2.1 Phase 1K.E.5+6 — Eq.
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
        panic!("Phase 1K.E.5+6 must compile the Eq chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 1);
}

/// v2.1 Phase 1K.E.5+6 — branchy chunk with multi-op then/else.
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
        panic!("Phase 1K.E.5+6 must compile the branchy chunk");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 10);
}

/// v2.1 Phase 1K.E.5+6 — branchy chunk, else path. n=20 → r = 20-1
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
        panic!("Phase 1K.E.5+6 must compile the branchy chunk (else)");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 19);
}

/// v2.1 Phase 1K.E.7 — dead-locals `local b = true` compiles
/// (`LoadTrue + Return0`).
#[test]
fn dead_locals_load_true_then_return0() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local b = true", b"=load_true_dead")
        .expect("compile");
    let proto = closure.proto;
    assert!(proto.code.iter().any(|i| i.op() == Op::LoadTrue));
    assert_eq!(proto.code.last().map(|i| i.op()), Some(Op::Return0));

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let result = backend.try_compile(&mut storage, proto, false, false);
    let CompileResult::Compiled {
        entry, returns_one, ..
    } = result
    else {
        panic!("Phase 1K.E.7 dead-locals LoadTrue chunk must compile; got {result:?}");
    };
    assert!(!returns_one, "Return0 chunks report returns_one=false");
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    // The chunk's `b = true` is dead at Return0; entry returns 0.
    assert_eq!(unsafe { entry_fn() }, 0);
}

/// v2.1 Phase 1K.E.7 — dead-locals `local b = false`.
#[test]
fn dead_locals_load_false_then_return0() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(b"local b = false", b"=load_false_dead")
        .expect("compile");
    let proto = closure.proto;
    assert!(proto.code.iter().any(|i| i.op() == Op::LoadFalse));

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("Phase 1K.E.7 dead-locals LoadFalse chunk must compile");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 0);
}

/// v2.1 Phase 1K.E.7 — `return true` is **out of scope** until the
/// dispatcher contract grows a `ret_is_bool` bit. The chunk must
/// bail rather than mis-encoding `true` as `i64(1)` (the dispatcher
/// would interpret that as `Value::Int(1)`).
#[test]
fn return_true_bails_until_bool_ret_widening() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(b"return true", b"=return_true").expect("compile");
    let proto = closure.proto;
    assert!(proto.code.iter().any(|i| i.op() == Op::LoadTrue));
    assert!(proto.code.iter().any(|i| i.op() == Op::Return1));

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let result = backend.try_compile(&mut storage, proto, false, false);
    assert!(
        matches!(result, CompileResult::Skipped),
        "Op::LoadTrue + Return1 must bail until dispatcher widening \
         (got {result:?})",
    );
}

/// v2.1 Phase 1K.E.8 — substantial int chunk that exercises every
/// non-call op the compute path supports as of Phase 1K.E:
/// `LoadI` / `Move` / `Add` / `Sub` / `Mul` / `Mod` / `Lt` / `Le` /
/// `Eq` / `Jmp` / `Return1`. Closest in-scope analog to the "fib(N)
/// shape" the Phase 1K.E plan referenced — proper recursive `fib`
/// needs `Op::Call` (1K.F helper-call emit) and iterative `fib` needs
/// `Op::ForPrep` / `Op::ForLoop`, both out of 1K.E scope.
///
/// Chunk:
/// ```lua
/// local n = 7
/// if n < 5 then
///   return n * n
/// else
///   local d = n - 3        -- 4
///   if d == 4 then
///     return d + 100       -- 104
///   else
///     return d % 3
///   end
/// end
/// ```
/// For n=7: 7<5 false → else; d = 4; d==4 true → return 104.
#[test]
fn fib_shape_nested_branchy_chunk() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local n = 7\n\
              if n < 5 then\n\
                return n * n\n\
              else\n\
                local d = n - 3\n\
                if d == 4 then\n\
                  return d + 100\n\
                else\n\
                  return d % 3\n\
                end\n\
              end",
            b"=fib_shape_nested",
        )
        .expect("compile");
    let proto = closure.proto;

    // Confirm the chunk really exercises the breadth of ops the test
    // claims. If the parser ever folds any of these out, the test
    // loses coverage — surface that loudly.
    for op in [
        Op::LoadI,
        Op::Lt,
        Op::Eq,
        Op::Mul,
        Op::Sub,
        Op::Mod,
        Op::Add,
        Op::Jmp,
        Op::Return1,
    ] {
        assert!(
            proto.code.iter().any(|i| i.op() == op),
            "fib-shape chunk must exercise {op:?}; code = {:?}",
            proto.code,
        );
    }

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled {
        entry, returns_one, ..
    } = backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("Phase 1K.E.8 fib-shape chunk must compile via compute path");
    };
    assert!(returns_one);
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(
        unsafe { entry_fn() },
        104,
        "n=7 → else → d=4 → d==4 → return 104",
    );
}

/// v2.1 Phase 1K.E.8 — same chunk, then-branch path (n=3):
/// `3 < 5` true → return `3 * 3` = 9.
#[test]
fn fib_shape_nested_branchy_chunk_then_path() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local n = 3\n\
              if n < 5 then\n\
                return n * n\n\
              else\n\
                local d = n - 3\n\
                if d == 4 then\n\
                  return d + 100\n\
                else\n\
                  return d % 3\n\
                end\n\
              end",
            b"=fib_shape_nested_then",
        )
        .expect("compile");
    let proto = closure.proto;
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("Phase 1K.E.8 then-branch chunk must compile");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 9);
}

/// v2.1 Phase 1K.E.8 — deepest else-branch (n=11 → else → d=8 →
/// d==4 false → return `d % 3 == 2`).
#[test]
fn fib_shape_nested_branchy_chunk_deep_else() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local n = 11\n\
              if n < 5 then\n\
                return n * n\n\
              else\n\
                local d = n - 3\n\
                if d == 4 then\n\
                  return d + 100\n\
                else\n\
                  return d % 3\n\
                end\n\
              end",
            b"=fib_shape_nested_deep_else",
        )
        .expect("compile");
    let proto = closure.proto;
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("Phase 1K.E.8 deep-else chunk must compile");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 2);
}

/// v2.1 Phase 1K.E.2 — `Move`-through-the-return-slot smoke.
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
        panic!("`local x = 9; local y = x; return y` must compile via Phase 1K.E.2");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(
        returned, 9,
        "Move must propagate the value to the return reg"
    );
}
