//! LLVM 18 + inkwell 0.10 alternative JIT backend for luna.
//!
//! `LlvmBackend` implements `IntChunkCompiler` + `TraceCompiler`.
//! Shapes the LLVM codegen does not handle return `Skipped` / `None`
//! so the dispatcher falls through to the interpreter.
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
mod storage;
mod trace;

pub use storage::LlvmJitStorage;

/// LLVM-backed JIT backend zero-sized type. Implements
/// `IntChunkCompiler` + `TraceCompiler`; shapes the codegen does not
/// handle return `Skipped` / `None`.
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

    #[allow(clippy::not_unsafe_ptr_arg_deref)] // Trait impl required by IntChunkCompiler; SAFETY contract documented in the body — caller is the dispatcher with a live `&mut Vm`. Matches `CraneliftBackend::enter`.
    fn enter(&self, vm: *mut Vm, cl: Option<Gc<LuaClosure>>) -> JitVmGuard {
        // Reuse the shared `enter_jit` from `luna-jit-helpers` so the
        // `JIT_VM` / `JIT_CL` TLS slots stay single-source-of-truth
        // across backends. Same RAII semantics as Cranelift's path.
        //
        // SAFETY: the dispatcher derived `vm` from a live `&mut Vm`;
        // the JIT entry that runs under the returned guard reaches
        // back into the Vm only through the TLS pointer installed
        // here (helpers read it via `JIT_VM`). Vm is `?Send` /
        // single-threaded; no aliasing concern within this entry.
        let vm_ref = unsafe { &mut *vm };
        luna_jit_helpers::enter_jit(vm_ref, cl)
    }
}

impl TraceCompiler for LlvmBackend {
    fn try_compile_trace(
        &self,
        storage: &mut dyn JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        // Delegate to the LLVM trace lowerer.
        // Down-cast `dyn JitStorage` to the concrete `LlvmJitStorage` so
        // `trace::try_compile_trace` can park the engine pair.
        let llvm_storage = storage.as_any_mut().downcast_mut::<LlvmJitStorage>()?;
        trace::try_compile_trace(llvm_storage, record, opts)
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        "llvm-1k-g"
    }
}
