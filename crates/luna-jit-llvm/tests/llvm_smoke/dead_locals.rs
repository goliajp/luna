//! Dead-locals chunks, the compile cache, and shapes the backend skips.

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

/// 3-op chunk smoke.
///
/// `local x; local y = 'h'; local z = y` compiles to
/// `[LoadNil(R0,0), LoadK(R1, "h"), Move(R2,R1), Return0]`. The
/// chunk's locals are dead at the `Return0` boundary, so the
/// recognised shape `[(LoadNil | LoadK | Move)*, Return0]` lowers
/// to the same `extern "C" fn() -> i64 { ret 0 }` entry as the
/// LoadNil-only chunk.
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

    // SAFETY: matches the LoadNil smoke's calling convention.
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    let returned = unsafe { entry_fn() };
    assert_eq!(returned, 0);
}

/// Verify the per-`Vm` `LlvmJitStorage` cache
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

/// Sanity check that out-of-shape chunks bail back to the
/// interpreter rather than being mis-compiled.
///
/// The compute path handles `LoadI` + `Return1`, so a chunk whose op
/// set falls outside *both* paths is needed for `Skipped` coverage.
/// `local t = {}` emits `NewTable` + `Return0`;
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

/// Dead-locals `local b = true` compiles
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
        panic!("dead-locals LoadTrue chunk must compile; got {result:?}");
    };
    assert!(!returns_one, "Return0 chunks report returns_one=false");
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    // The chunk's `b = true` is dead at Return0; entry returns 0.
    assert_eq!(unsafe { entry_fn() }, 0);
}

/// Dead-locals `local b = false`.
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
        panic!("dead-locals LoadFalse chunk must compile");
    };
    let entry_fn: unsafe extern "C" fn() -> i64 =
        unsafe { std::mem::transmute::<*const u8, _>(entry) };
    assert_eq!(unsafe { entry_fn() }, 0);
}

/// `return true` is **out of scope** until the
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
