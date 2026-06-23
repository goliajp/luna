//! luna-core JIT surface — **trait + types only, zero Cranelift**.
//!
//! The interpreter dispatcher in [`crate::vm::exec`] routes JIT calls
//! through [`IntChunkCompiler`] / [`TraceCompiler`] trait objects;
//! luna-core ships only the no-op [`NullJitBackend`], which makes the
//! whole crate Cranelift-free (and therefore zero-three-party-dep,
//! per v1.1 F1).
//!
//! The Cranelift-backed implementations (`CraneliftBackend`, the 26
//! `luna_jit_*` extern "C" helpers, the `JIT_CACHE` thread-local,
//! `enter_jit`, `cache_lookup_or_compile`, `try_compile_int_chunk`,
//! `try_compile_trace_with_options`, …) live in the `luna` crate
//! under `luna::jit_backend`. Embedders who want the JIT use
//! `luna::Vm::new_minimal_with_jit(version)` instead of
//! `luna_core::vm::Vm::new_minimal(version)`.

// v1.1 A1 Session B — pure data types + small cranelift-free helpers
// for the trace JIT live here. Session C moves the file under
// luna-core unchanged.
pub mod trace_types;
pub use trace_types::*;

// v1.1 A1 Session A — backend trait surface introduced in-place.
// Session C moves it to luna-core (this file) and extracts the
// Cranelift-bound CraneliftBackend struct out into luna's
// jit_backend module.
mod abi;
pub use abi::{
    CompileResult, IntChunkCompiler, IntChunkFn, IntFn1, IntFn2, IntFn3, IntFn4, MAX_JIT_ARITY,
    NullJitBackend, TraceCompiler,
};

// Compatibility re-export so external `use luna::jit::trace::*` paths
// (and the historical `crate::jit::trace::*` accesses inside this
// crate) keep resolving even after Session C's physical split. In
// luna-core `trace` is just a namespace alias for `trace_types`; in
// luna it's enriched with codegen-bearing items via a different
// re-export.
/// Trace-JIT types namespace. In `luna-core` this is a thin re-export of
/// [`trace_types`]; the `luna` crate enriches it with the Cranelift-backed
/// codegen entry points (`try_compile_trace`, etc.).
pub mod trace {
    pub use super::trace_types::*;
}

/// Construct an inert [`JitVmGuard`] that performs neither TLS install
/// nor TLS clear. Used by [`NullJitBackend::enter`]: since
/// `try_compile` / `try_compile_trace` always skip work, no JIT mcode
/// ever fires and no helper consults the TLS slots, so the guard only
/// has to keep the trait method signature symmetric with luna's
/// `CraneliftBackend::enter`. [`JitVmGuard`]'s drop is itself a no-op
/// (see `impl Drop for JitVmGuard`).
#[inline]
pub fn noop_jit_guard() -> JitVmGuard {
    JitVmGuard { _private: () }
}

/// P11-S5c — RAII guard pinning the active `Vm` (and optional closure)
/// pointer for JIT-emitted Rust helper calls. The Cranelift backend's
/// `enter_jit` (in the luna crate) installs the thread-local slots;
/// drop is a no-op (TLS values are overwritten on the next enter, so
/// helpers running outside a fresh `enter_jit` would already trip the
/// debug-assert).
///
/// Path C #7 — drop is a no-op. JIT_VM / JIT_CL get overwritten
/// on next `enter_jit` call. Helpers inside JIT mcode only ever
/// run while a guard is alive (dispatcher calls `enter_jit` then
/// the entry_fn, helpers fire inside entry_fn, returns to dispatcher
/// BEFORE guard drops). Between dispatches the TLS values are
/// stale but interp loop doesn't read them — only JIT-mcode-
/// invoked helpers do, and those run only INSIDE a fresh `enter_jit`.
///
/// Skipping the 2 TLS writes per dispatch (~5-10 cycles each on arm64
/// thread_local fastpath) saves ~1.5 ms on fib_28 (434k dispatches ×
/// ~10 cycles).
///
/// Debug-mode assertions still catch misuse via `current_jit_vm`'s
/// is_null check IF a helper runs outside `enter_jit` (would happen
/// only if some bug calls a helper symbol from interp code, which
/// never happens by design).
#[must_use = "the guard must outlive the JIT entry call"]
pub struct JitVmGuard {
    /// Field is constructed only by the two RAII entry points
    /// (`noop_jit_guard` here and `enter_jit` in luna). Kept
    /// `pub(crate)` so luna's `jit_backend` can also build one.
    pub(crate) _private: (),
}

impl Drop for JitVmGuard {
    fn drop(&mut self) {
        // See struct doc for the rationale: no-op by design.
    }
}
