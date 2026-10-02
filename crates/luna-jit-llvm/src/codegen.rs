//! LLVM int-chunk codegen.
//!
//! Two recognised paths land here:
//!
//! 1. **Dead-locals path** —
//!    `[(LoadNil | LoadK | Move)*, Return0, ...]` chunks whose locals
//!    are unobservable at the `Return0` boundary. Emit shrinks to
//!    `extern "C" fn() -> i64 { ret 0 }` because no JIT-entry caller
//!    ever reads the locals (`returns_one = false`). This path covers
//!    chunks like `local x` / `local y = 'h'; local z = y` — including
//!    LoadK of *string* / *bool* / *nil* constants whose value the
//!    interpreter would compute but the JIT entry can elide.
//!
//! 2. **Compute path** — chunks that reach an
//!    observable `Return1 R[A]`. The lowerer builds an `[N x i64]`
//!    register file on entry, emits one LLVM IR instruction per
//!    recognised op, and tails into either `ret i64 0` (Return0) or
//!    `ret <reg>` (Return1).
//!
//! ## Why two paths instead of one
//!
//! The dead-locals path can lower **chunks containing LoadK of
//! non-int constants** (string / bool / nil) because the values are
//! never observed. The compute path can only lower ops whose
//! semantics it actually emits — so it bails on string / bool / nil
//! LoadK. Keeping both paths lets the compute whitelist grow op-by-op
//! without breaking chunk shapes the dead-locals path already covers.
//!
//! ## Cache key
//!
//! Both paths share the key over the bytecode words and the numeric
//! constants (see `proto_cache_key`): the constant-operand ops
//! (`AddK`, `EqK`, …) read `proto.consts`, so two protos with the same
//! code but different constants must not share native code.
//!
//! ## Lifetime path
//!
//! See [`super::storage`] module docs for the `'ctx` lifetime
//! reasoning. Each compile boxes a fresh `Context` and the engine
//! borrows it for `'static` via a localised pointer upgrade; the
//! pair lands in `LlvmJitStorage::engines` and gets dropped together
//! when the Vm drops.

use crate::storage::{CachedEntry, EnginePair, LlvmJitStorage};
use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::values::FunctionValue;
use luna_core::jit::{CompileResult, JitStorage};
use luna_core::runtime::{Gc, Value, function::Proto};
use luna_core::vm::isa::{Inst, Op};
use std::collections::HashMap;
use std::hash::Hasher;

mod compute;
mod emitter;
mod flow;
mod helpers;
mod plan;

use compute::compile_compute_chunk;
use helpers::bind_helper_symbols;
pub(crate) use helpers::declare_jit_helpers;
use plan::ChunkPlan;

/// Try to lower `proto` to native code
/// via LLVM. Returns `None` when the body falls outside the recognised
/// shape (caller turns into `CompileResult::Skipped`).
///
/// Recognised shapes:
/// - **Dead locals**: a (possibly empty) prefix of
///   `LoadNil | LoadK | Move` followed by `Return0`. Lowers to
///   `extern "C" fn() -> i64 { ret 0 }`.
/// - **Compute**: a (possibly empty) prefix of
///   `LoadI | LoadNil | LoadK(Int) | Move` followed by either
///   `Return1` or `Return0`. Lowers to a per-op reg-array entry.
///
/// `pre53` is fed into the cache key for ABI parity with Cranelift's
/// `proto_cache_key`; the recognised shapes currently don't touch the
/// dialect-bit semantics.
pub(crate) fn try_compile_int_chunk(
    storage: &mut dyn JitStorage,
    proto: Gc<Proto>,
    pre53: bool,
) -> Option<CompileResult> {
    // Path 1: dead-locals fast path. Restricted to
    // zero-param chunks because its `extern "C" fn() -> i64` emit
    // signature doesn't accept positional args.
    if proto.num_params == 0 && is_dead_locals_then_return0(&proto.code) {
        return Some(via_cache(storage, &proto, pre53, 0, false, &|| {
            compile_constant_zero_chunk()
        }));
    }

    // Path 2: compute path. Scans for the cumulative whitelist + the
    // chunk's effective return shape, then lowers op-by-op into a
    // reg-array entry. Parametric chunks land as
    // `extern "C" fn(i64, …, i64) -> i64` with arg-load shims in the
    // entry BB.
    if let Some(plan) = ChunkPlan::from_proto(&proto) {
        let num_params = plan.num_params as u8;
        return Some(via_cache(
            storage,
            &proto,
            pre53,
            num_params,
            plan.returns_one,
            &|| compile_compute_chunk(&plan),
        ));
    }

    None
}

/// Shared cache+compile wrapper. Looks up the proto in the storage
/// cache; on a hit returns the cached entry, on a miss invokes
/// `compile_fn`, parks the resulting `(EngineEntry, EnginePair)` on
/// the cache, and returns the new entry. `returns_one` is baked into
/// the `CachedEntry` so a hit on a subsequent compile reproduces the
/// same dispatcher contract.
fn via_cache(
    storage: &mut dyn JitStorage,
    proto: &Proto,
    pre53: bool,
    num_args: u8,
    returns_one: bool,
    compile_fn: &dyn Fn() -> Option<(*const u8, EnginePair)>,
) -> CompileResult {
    let store = storage
        .as_any_mut()
        .downcast_mut::<LlvmJitStorage>()
        .expect("LlvmBackend installed without LlvmJitStorage");
    let key = proto_cache_key(proto, pre53);
    if let Some(hit) = store.cache.get(&key).copied() {
        return hit.to_compile_result();
    }
    let Some((entry_ptr, pair)) = compile_fn() else {
        return CompileResult::Skipped;
    };
    let cached = CachedEntry {
        entry: entry_ptr,
        num_args,
        returns_one,
        arg_float_mask: 0,
        arg_table_mask: 0,
        ret_is_float: false,
        ret_is_table: false,
    };
    store.insert(key, pair, cached);
    cached.to_compile_result()
}

/// Accept
/// `[(LoadNil | LoadK | Move | LoadFalse | LoadTrue)*, Return0, ...]`.
///
/// The dead-locals path eats every load op whose effect is invisible
/// at the `Return0` boundary, regardless of what value the load
/// produces — strings, booleans, nils, and any LoadK constant kind
/// all qualify because the chunk returns no value. The trailing
/// implicit `Return0` the parser emits after a `Return1` is not
/// relevant here — we only fire on a chunk whose *first* reachable
/// return is `Return0`.
///
/// `LoadFalse` / `LoadTrue` are accepted on the dead-locals path
/// **only**; the compute path needs a dispatcher widening
/// (`ret_is_bool` bit) before `return true` / `return false` can
/// flow through the JIT-entry → caller contract without misreading
/// the i64 as an integer; until then a chunk like `return true`
/// falls through to the interpreter.
fn is_dead_locals_then_return0(code: &[Inst]) -> bool {
    let Some(first_ret_pc) = code
        .iter()
        .position(|i| matches!(i.op(), Op::Return0 | Op::Return1))
    else {
        return false;
    };
    if code[first_ret_pc].op() != Op::Return0 {
        return false;
    }
    code[..first_ret_pc].iter().all(|i| {
        matches!(
            i.op(),
            Op::LoadNil | Op::LoadK | Op::Move | Op::LoadFalse | Op::LoadTrue
        )
    })
}

/// Stable cache key for a Proto: the bytecode words, the numeric
/// constants (the `K` forms fold them into the code) and the dialect
/// bit.
fn proto_cache_key(proto: &Proto, pre53: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for inst in proto.code.iter() {
        h.write_u32(inst.0);
    }
    for c in proto.consts.iter() {
        match c {
            Value::Int(i) => {
                h.write_u8(0);
                h.write_i64(*i);
            }
            Value::Float(f) => {
                h.write_u8(1);
                h.write_u64(f.to_bits());
            }
            _ => h.write_u8(2),
        }
    }
    h.write_u8(pre53 as u8);
    h.finish()
}

/// Build the dead-locals JIT entry: `extern "C" fn() -> i64 { ret 0 }`.
/// Returns the entry pointer + owning `(Context, ExecutionEngine)`
/// pair so the caller can park it on storage. Used by the dead-locals
/// fast path; the compute path uses [`compile_compute_chunk`].
fn compile_constant_zero_chunk() -> Option<(*const u8, EnginePair)> {
    let ctx_box: Box<Context> = Box::new(Context::create());
    // SAFETY: see `compile_compute_chunk` below for the full lifetime
    // discussion; the same reasoning applies — ctx_box outlives the
    // engine via `EnginePair`'s field-order drop discipline.
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

    finalize_module(ctx_box, module, None)
}

/// Shared module-finalisation tail. JIT-compiles `module` under the
/// (heap-pinned) `ctx_box`, resolves the entry symbol, and bundles
/// both into an [`EnginePair`] for storage.
///
/// `helpers` is `Some(map)` for paths that may invoke `luna_jit_*`
/// helpers (the compute path) — every entry gets bound to its real
/// Rust function address via `add_global_mapping`. The dead-locals
/// path passes `None` because its IR makes no
/// helper calls and skipping the map keeps that fast-path tight.
/// Also used by the trace JIT (`trace.rs`).
pub(crate) fn finalize_module<'ctx>(
    ctx_box: Box<Context>,
    module: Module<'ctx>,
    helpers: Option<&HashMap<&'static str, FunctionValue<'ctx>>>,
) -> Option<(*const u8, EnginePair)> {
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::None)
        .ok()?;
    if let Some(map) = helpers {
        bind_helper_symbols(&engine, map);
    }
    let entry_ptr = engine.get_function_address("luna_jit_llvm_entry").ok()? as *const u8;
    // SAFETY: `EnginePair` holds the engine as
    // `ExecutionEngine<'static>` — see the constructor of
    // `compile_compute_chunk` for the lifetime upgrade discussion.
    // The transmute below relabels `EE<'ctx>` to `EE<'static>`; the
    // `'ctx` borrow stays live for the engine's observable lifetime
    // because `ctx_box` is moved into the pair on the same line and
    // pinned by struct field-order drop.
    let engine_static: inkwell::execution_engine::ExecutionEngine<'static> =
        unsafe { std::mem::transmute(engine) };
    let pair = EnginePair {
        engine: engine_static,
        context: ctx_box,
    };
    Some((entry_ptr, pair))
}
