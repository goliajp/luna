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
