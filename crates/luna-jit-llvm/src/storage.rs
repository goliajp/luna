//! v2.1 Phase 1K.D.5 — LLVM backend storage marker.
//!
//! Phase 1K.D.5 marker only — holds nothing; the trait stub's
//! `try_compile` always returns `Skipped` so no downcast site is
//! reached. Phase 1K.D.8 wires the real cache + handle collections
//! (one `inkwell::execution_engine::ExecutionEngine` per compiled
//! chunk, mirroring `CraneliftJitStorage`'s `JITModule`-per-compile
//! shape). The Risk #1 spike from `.dev/rfcs/v2.1-phase-1k-c-trait-
//! audit.md` § 6 covers the `'ctx` lifetime + per-Vm ownership
//! question that determines the concrete shape.

use luna_core::jit::JitStorage;

/// LLVM-side per-`Vm` JIT storage. Phase 1K.D.5 marker; Phase 1K.D.8
/// extends with the cache + ExecutionEngine handle collection.
#[derive(Default)]
pub struct LlvmJitStorage;

impl JitStorage for LlvmJitStorage {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
