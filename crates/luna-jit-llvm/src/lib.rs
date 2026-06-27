//! v2.1 Phase 1K.D — LLVM 18 + inkwell 0.9 alternative JIT backend
//! for luna.
//!
//! ## Status
//!
//! Phase 1K.D.5 stub: `LlvmBackend` ZST implements
//! `IntChunkCompiler` + `TraceCompiler` with every method returning
//! `Skipped` / `None` (same shape as `luna_core::jit::NullJitBackend`)
//! so the dispatcher falls through to the interpreter without
//! changing behaviour. `LlvmJitStorage` is a marker-only struct.
//! Phase 1K.D.6+ replaces the trait stubs with actual LLVM codegen
//! one op at a time.
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
//! See `.dev/rfcs/v2.1-phase-1k-c-trait-audit.md` § 4.3 for the
//! end-to-end selection design.
//!
//! ## Why a separate crate (vs. a luna-jit submodule)
//!
//! The 1K.C audit (§ 3.5) requires the two backends to ship in
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

pub use storage::LlvmJitStorage;

/// v2.1 Phase 1K.D.2 — LLVM-backed JIT backend zero-sized type.
/// Implements `IntChunkCompiler` + `TraceCompiler`; trait method
/// bodies live as stubs through Phase 1K.D.5 (everything returns
/// `Skipped` / `None`) and grow real LLVM codegen op-by-op starting
/// at Phase 1K.D.6.
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
        // v2.1 Phase 1K.D.6 — Op::LoadNil smoke. Other shapes still
        // bail through `CompileResult::Skipped`; see `codegen` module.
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
        _storage: &mut dyn JitStorage,
        _record: &TraceRecord,
        _opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        // Phase 1K.D ships method-JIT-only LLVM. Trace-JIT lowering
        // lands in Phase 1K.F (one-shape, no side-exits) and 1K.G
        // (side-exits + helper-call emit). Until then traces always
        // bail back to the interpreter, matching the trait contract.
        None
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        "llvm-stub"
    }
}
