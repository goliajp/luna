//! LLVM 18 + inkwell 0.10 alternative JIT backend for luna.
//!
//! `LlvmBackend` implements `IntChunkCompiler` (the method JIT); shapes
//! the LLVM codegen does not handle return `Skipped` so the dispatcher
//! falls through to the interpreter. Traces are lowered by luna-jit's
//! trace lowerer, shared with the Cranelift backend, and compiled through
//! [`compile_function`]; `LlvmBackend` on its own compiles no traces.
//!
//! ## How luna selects this backend
//!
//! 1. `luna-jit` is built with `--features llvm-jit` (default OFF;
//!    keeps Cranelift as the default and avoids dragging LLVM into
//!    the standard install).
//! 2. At runtime the `LUNA_JIT_BACKEND=llvm` env var flips
//!    `luna_jit::install_default_jit` from `CraneliftBackend` to
//!    `LlvmBackend`.
//!
//!
//! ## Why a separate crate (vs. a luna-jit submodule)
//!
//! The two backends have to ship in
//! independent crates so:
//! - Cranelift is not pulled into the LLVM-only install path.
//! - LLVM is not pulled into the default Cranelift install path
//!   (LLVM adds ~1 GB of system deps on the dev host).
//! - Both backends register the *same* `luna_jit_*` symbol set via
//!   the shared `luna-jit-helpers` crate (single-source-of-truth
//!   for helper definitions).

use luna_core::jit::{
    CompileResult, IntChunkCompiler, JitStorage, JitVmGuard, TraceCompiler,
    trace_types::{CompileOptions, CompiledTrace, TraceRecord},
};
use luna_core::runtime::{Gc, LuaClosure, function::Proto};
use luna_core::vm::Vm;

mod codegen;
mod function;
mod job;
mod operands;
mod storage;
mod upval_roles;

pub use function::{ENTRY, compile_function};
pub use job::{ChunkJob, CompiledChunk};
pub use storage::{EnginePair, LlvmJitStorage};

/// LLVM-backed JIT backend zero-sized type. Implements
/// `IntChunkCompiler`; shapes the codegen does not handle return
/// `Skipped`. Its `TraceCompiler` compiles nothing: luna-jit's
/// `LUNA_JIT_BACKEND=llvm` backend compiles traces with LLVM.
#[derive(Default, Clone, Copy)]
pub struct LlvmBackend;

impl IntChunkCompiler for LlvmBackend {
    fn try_compile(
        &self,
        storage: &mut dyn JitStorage,
        proto: Gc<Proto>,
        pre53: bool,
        _float_only: bool,
    ) -> CompileResult {
        // Unsupported shapes bail through `CompileResult::Skipped`;
        // see the `codegen` module.
        match codegen::try_compile_int_chunk(storage, proto, pre53) {
            Some(c) => c,
            None => CompileResult::Skipped,
        }
    }

    fn enter(&self, vm: *mut Vm, cl: Option<Gc<LuaClosure>>) -> JitVmGuard {
        // Reuse the shared entry from `luna-jit-helpers` so the
        // `JIT_VM` / `JIT_CL` TLS slots stay single-source-of-truth
        // across backends. Same RAII semantics as Cranelift's path.
        luna_jit_helpers::enter_jit_ptr(vm, cl)
    }
}

impl TraceCompiler for LlvmBackend {
    fn try_compile_trace(
        &self,
        _storage: &mut dyn JitStorage,
        _record: &TraceRecord,
        _opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        None
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        "llvm:traces-need-luna-jit"
    }
}
