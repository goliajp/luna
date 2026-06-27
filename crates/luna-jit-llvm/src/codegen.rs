//! v2.1 Phase 1K.D — LLVM int-chunk codegen.
//!
//! Phase 1K.D.6 lights up `Op::LoadNil` end-to-end (chunk shape
//! `[LoadNil(_, _), Return0]`); the JIT entry fn signature is
//! `unsafe extern "C" fn() -> i64` returning the chunk's "no value"
//! sentinel (`0`). Phase 1K.D.7 extends to `Op::LoadK + Op::Move +
//! Op::LoadNil`. Phase 1K.E grows out to the full Cranelift
//! int-chunk whitelist.
//!
//! ## Lifetime management (Phase 1K.D.6 placeholder)
//!
//! `inkwell::Context` borrows from itself (`<'ctx>`), and
//! `ExecutionEngine<'ctx>` borrows from it in turn — meanwhile the
//! entry pointer the trait returns must outlive `try_compile`'s
//! stack frame so the dispatcher can keep calling it across
//! subsequent Vm operations. Phase 1K.D.6 takes the simplest path:
//! `Box::leak` both the `Context` and the `ExecutionEngine` per
//! compile so the JIT mcode mmap stays alive for the process
//! lifetime. Phase 1K.D.8 replaces this with the per-Vm
//! `LlvmJitStorage` cache (Risk #1 in
//! `.dev/rfcs/v2.1-phase-1k-c-trait-audit.md` § 6 — the `'ctx`
//! lifetime + ouroboros / transmute trade-off is decided there).
//! Until then the leak is acceptable in a smoke phase because each
//! Proto compiles at most once per process; the unbounded growth
//! is bounded by the test/proto count.

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use luna_core::jit::{CompileResult, JitStorage};
use luna_core::runtime::{Gc, function::Proto};
use luna_core::vm::isa::Op;

/// v2.1 Phase 1K.D.6 — try to lower `proto` to native code via LLVM.
///
/// Recognised shapes (cumulative across 1K.D.6 → 1K.D.7):
///
/// - `[Op::LoadNil(_, _), Op::Return0]` — emit
///   `extern "C" fn() -> i64 { ret 0 }`. The destination register
///   write is a no-op at the JIT level: there's no value to return
///   from the chunk, and the only Lua-observable side-effect (the
///   register `nil`-fill) is irrelevant once the chunk returns
///   without exposing its locals.
///
/// Out-of-shape Protos return `None`, which the trait impl turns
/// into `CompileResult::Skipped` so the interpreter handles them.
///
/// `pre53` is accepted for API symmetry with the Cranelift backend
/// but currently ignored: no recognised shape touches the
/// `for`-loop dialect bit.
#[allow(unused_variables)]
pub(crate) fn try_compile_int_chunk(
    storage: &mut dyn JitStorage,
    proto: Gc<Proto>,
    pre53: bool,
) -> Option<CompileResult> {
    // `proto` was passed by the dispatcher via the trait; the Gc is
    // rooted for the duration of this call. `Gc<T>: Deref<Target=T>`
    // so `&proto.code` reaches the bytecode slice directly.
    if !is_load_nil_then_return0(&proto.code) {
        return None;
    }
    let entry = compile_constant_zero_chunk()?;
    Some(CompileResult::Compiled {
        entry,
        num_args: 0,
        returns_one: false,
        arg_float_mask: 0,
        arg_table_mask: 0,
        ret_is_float: false,
        ret_is_table: false,
    })
}

/// Pattern-match the Phase 1K.D.6 chunk shape: `[LoadNil(_, _),
/// Return0]`. Future phases (1K.D.7+) layer additional shapes on
/// top of this.
fn is_load_nil_then_return0(code: &[luna_core::vm::isa::Inst]) -> bool {
    match code {
        [a, b] => a.op() == Op::LoadNil && b.op() == Op::Return0,
        _ => false,
    }
}

/// Build, JIT-compile, and return the entry pointer for
/// `extern "C" fn() -> i64 { ret 0 }`. Box-leaks the `Context` and
/// the `ExecutionEngine` so the mmap stays callable for the process
/// lifetime (see module-level docs on the lifetime path planned for
/// Phase 1K.D.8).
fn compile_constant_zero_chunk() -> Option<*const u8> {
    // The Context must be `'static` because the JitFunction returned
    // by `get_function` borrows from it. `Box::leak` is the simplest
    // way to lift a per-compile Context to `'static` without the
    // self-referential `ouroboros` dep — Phase 1K.D.8 will park the
    // pair on the per-Vm storage cache instead.
    let ctx: &'static Context = Box::leak(Box::new(Context::create()));

    let module = ctx.create_module("luna_jit_llvm_loadnil");
    let builder = ctx.create_builder();

    let i64_type = ctx.i64_type();
    let fn_type = i64_type.fn_type(&[], false);
    let function = module.add_function("luna_jit_llvm_entry", fn_type, None);
    let entry_block = ctx.append_basic_block(function, "entry");
    builder.position_at_end(entry_block);

    let zero = i64_type.const_int(0, false);
    builder.build_return(Some(&zero)).ok()?;

    let ee = module
        .create_jit_execution_engine(OptimizationLevel::None)
        .ok()?;
    let entry_ptr = ee.get_function_address("luna_jit_llvm_entry").ok()? as *const u8;

    // Leak the EE so the JIT-compiled mcode mmap stays alive past
    // this scope. Phase 1K.D.8's per-Vm storage cache replaces this.
    let _leaked: &'static ExecutionEngine<'static> = Box::leak(Box::new(ee));

    Some(entry_ptr)
}
