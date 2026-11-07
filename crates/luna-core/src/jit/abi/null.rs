//! The backend that compiles nothing.

use super::*;

/// No-op backend installed by [`crate::vm::Vm::new_minimal`] in
/// luna-core. Both calls report "nothing compiled" so the interp
/// dispatcher always takes the standard path. Embedders who want a
/// real JIT either depend on the `luna` crate (whose
/// `Vm::new_minimal_with_jit` swaps in `CraneliftBackend`) or write
/// their own `IntChunkCompiler` / `TraceCompiler` implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullJitBackend;

// SAFETY: it compiles nothing
unsafe impl IntChunkCompiler for NullJitBackend {
    fn try_compile(
        &self,
        _: &mut dyn crate::jit::JitStorage,
        _: Gc<Proto>,
        _: bool,
        _: bool,
    ) -> CompileResult {
        CompileResult::Skipped
    }

    fn enter(&self, _: *mut crate::vm::Vm, _: Option<Gc<LuaClosure>>) -> JitVmGuard {
        // The TLS slots stay whatever they were — no JIT mcode will
        // execute under a NullJitBackend (try_compile returned
        // Skipped, so no Proto reaches JitProtoState::Compiled, and
        // try_compile_trace returns None so no CompiledTrace ever
        // dispatches), so the helpers never read those TLS values.
        // JitVmGuard's drop is already a no-op (see the comment on
        // `JitVmGuard::drop` in `mod.rs`).
        crate::jit::noop_jit_guard()
    }
}

// SAFETY: it compiles nothing
unsafe impl TraceCompiler for NullJitBackend {
    fn try_compile_trace(
        &self,
        _: &mut dyn crate::jit::JitStorage,
        _: &TraceRecord,
        _: CompileOptions,
    ) -> Option<CompiledTrace> {
        None
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        "null-backend"
    }
}
