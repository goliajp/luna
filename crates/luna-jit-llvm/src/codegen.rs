//! v2.1 Phase 1K.D — LLVM int-chunk codegen.
//!
//! Phase 1K.D.6 lit up `Op::LoadNil` end-to-end (chunk shape
//! `[LoadNil(_, _), Return0]`); Phase 1K.D.7 extended to
//! `[(LoadNil | LoadK | Move)*, Return0]`. Phase 1K.D.8 wires the
//! per-Vm `LlvmJitStorage` cache so a second compile of the same
//! Proto hits the cache rather than re-emitting LLVM IR.
//!
//! Phase 1K.E grows out to ops with observable side effects
//! (`Op::Add`, `Op::Return1`, table ops, etc.); at that point the
//! JIT entry stops being a constant-zero stub and starts honouring
//! the per-op semantics.
//!
//! ## Cache key
//!
//! Same shape as `luna_jit::jit_backend::proto_cache_key` —
//! FNV-1a-64 over the bytecode words + the `pre53` dialect bit.
//! Constants are *not* fed in for Phase 1K.D because the only
//! constant the recognised shape ever touches is a string referenced
//! by `LoadK` whose value is unobservable across `Return0`. Phase
//! 1K.E (which lights up ops with observable consts like `Add R, K`)
//! will widen the key to include the constant payload.
//!
//! ## Lifetime path
//!
//! See [`super::storage`] module docs for the `'ctx` lifetime
//! reasoning. Phase 1K.D.8 picks one `Box::leak`-free `Context` per
//! compile so the resulting `ExecutionEngine<'static>` lands in
//! `LlvmJitStorage::engines` without lifetime gymnastics.

use crate::storage::{CachedEntry, EnginePair, LlvmJitStorage};
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use luna_core::jit::{CompileResult, JitStorage};
use luna_core::runtime::{Gc, function::Proto};
use luna_core::vm::isa::Op;
use std::hash::Hasher;

/// v2.1 Phase 1K.D.7 — try to lower `proto` to native code via LLVM.
///
/// Recognised shape: any chunk whose body is a (possibly empty)
/// prefix of `Op::LoadNil` / `Op::LoadK` / `Op::Move` followed by
/// `Op::Return0`. Every such chunk lowers to the constant-zero
/// entry fn: the prefix ops only touch Lua locals that are
/// discarded when the chunk returns without an observable value,
/// so the JIT entry can skip them entirely.
///
/// Out-of-shape Protos return `None`, which the trait impl turns
/// into `CompileResult::Skipped` so the interpreter handles them.
///
/// `pre53` is fed into the cache key (matches Cranelift's
/// `proto_cache_key` for ABI parity) but currently ignored
/// otherwise — no recognised shape touches the `for`-loop dialect
/// bit.
pub(crate) fn try_compile_int_chunk(
    storage: &mut dyn JitStorage,
    proto: Gc<Proto>,
    pre53: bool,
) -> Option<CompileResult> {
    if !is_dead_locals_then_return0(&proto.code) {
        return None;
    }
    let store = storage
        .as_any_mut()
        .downcast_mut::<LlvmJitStorage>()
        .expect("LlvmBackend installed without LlvmJitStorage");
    let key = proto_cache_key(&proto, pre53);
    if let Some(hit) = store.cache.get(&key).copied() {
        return Some(hit.to_compile_result());
    }
    let (entry_ptr, pair) = compile_constant_zero_chunk()?;
    let cached = CachedEntry {
        entry: entry_ptr,
        num_args: 0,
        returns_one: false,
        arg_float_mask: 0,
        arg_table_mask: 0,
        ret_is_float: false,
        ret_is_table: false,
    };
    store.insert(key, pair, cached);
    Some(cached.to_compile_result())
}

/// Phase 1K.D.7 — accept `[(LoadNil | LoadK | Move)*, Return0]`.
fn is_dead_locals_then_return0(code: &[luna_core::vm::isa::Inst]) -> bool {
    let Some((last, prefix)) = code.split_last() else {
        return false;
    };
    if last.op() != Op::Return0 {
        return false;
    }
    prefix
        .iter()
        .all(|i| matches!(i.op(), Op::LoadNil | Op::LoadK | Op::Move))
}

/// Phase 1K.D.8 — stable cache key for a Proto. FNV-1a-64 over the
/// bytecode words + the dialect bit. See module-level doc for why
/// constants aren't fed in for Phase 1K.D.
fn proto_cache_key(proto: &Proto, pre53: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for inst in proto.code.iter() {
        h.write_u32(inst.0);
    }
    h.write_u8(pre53 as u8);
    h.finish()
}

/// Build, JIT-compile, and return the entry pointer + owning
/// (Context, ExecutionEngine) pair for `extern "C" fn() -> i64 {
/// ret 0 }`. Caller parks the pair on the storage cache so the JIT
/// mmap survives the function's stack frame.
fn compile_constant_zero_chunk() -> Option<(*const u8, EnginePair)> {
    let ctx_box: Box<Context> = Box::new(Context::create());
    // SAFETY: `ctx_box` is heap-allocated and never moved out of
    // the `EnginePair` it ends up in; the engine borrows from `*ctx`
    // through the static reference, and the pair's drop order
    // (engine then context — declaration order matches that) keeps
    // the borrow valid for the engine's lifetime. The `'static`
    // lifetime is a localised lie that holds for the engine's
    // observable lifetime because the context outlives it.
    let ctx_static: &'static Context = unsafe { &*(ctx_box.as_ref() as *const Context) };

    let module = ctx_static.create_module("luna_jit_llvm_dead_locals");
    let builder = ctx_static.create_builder();

    let i64_type = ctx_static.i64_type();
    let fn_type = i64_type.fn_type(&[], false);
    let function = module.add_function("luna_jit_llvm_entry", fn_type, None);
    let entry_block = ctx_static.append_basic_block(function, "entry");
    builder.position_at_end(entry_block);
    let zero = i64_type.const_int(0, false);
    builder.build_return(Some(&zero)).ok()?;

    let engine = module
        .create_jit_execution_engine(OptimizationLevel::None)
        .ok()?;
    let entry_ptr = engine.get_function_address("luna_jit_llvm_entry").ok()? as *const u8;

    let pair = EnginePair {
        engine,
        context: ctx_box,
    };
    Some((entry_ptr, pair))
}
