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

// v1.3 Phase AOT Stage 7 sub-piece 4 — wire format for AOT trace
// metadata (header + index entry + encode/decode). Pure data; both
// `luna-aot` (compile-time encode) and `luna-runtime-helpers`
// (deploy-time decode) depend on this single module so the on-disk
// shape stays in lock-step.
pub mod aot_meta;

// v1.1 A1 Session A — backend trait surface introduced in-place.
// Session C moves it to luna-core (this file) and extracts the
// Cranelift-bound CraneliftBackend struct out into luna's
// jit_backend module.
mod abi;
pub use abi::{
    CompileResult, IntChunkCompiler, IntChunkFn, IntFn1, IntFn2, IntFn3, IntFn4, MAX_JIT_ARITY,
    NullJitBackend, TraceCompiler,
};

// v2.0 Track J sub-step J-B — per-Vm JIT storage trait. Holds the
// (formerly thread-local) JIT cache + handle collections so a Vm
// carries its own JIT state across thread moves (Send-prep for J-D).
mod storage;
pub use storage::{JitStorage, NullJitStorage};

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
/// `CraneliftBackend::enter`. The inert guard carries no restore
/// callback, so its drop is a no-op.
#[inline]
pub fn noop_jit_guard() -> JitVmGuard {
    JitVmGuard { restore: None }
}

/// P11-S5c — RAII guard pinning the active `Vm` (and optional closure)
/// pointer for JIT-emitted Rust helper calls.
///
/// # v2.0 Track J sub-step J-D — real RAII rebind
///
/// Prior to J-D, this guard's drop was a deliberate no-op: the
/// dispatcher always re-installed `JIT_VM` / `JIT_CL` on every
/// `enter_jit` call, so the previous-dispatch values would simply be
/// overwritten on the next entry. That trick saved 2 TLS writes per
/// dispatch (~5-10 cycles each on arm64) — measurable on fib_28's
/// 434k dispatches (~1.5 ms aggregate).
///
/// Track J Option B targets cross-thread Vm move under
/// `feature = "send"` (J-E flip). Once a Vm parks on thread A, moves
/// to thread B, and dispatches there, the per-thread `JIT_VM` slot on
/// thread B is null/stale at entry — `enter_jit` installs it fine on
/// the way in, but on the way out the no-op drop left stale Vm
/// pointers behind for any **nested** JIT entry (Lua-from-Rust-from-
/// JIT call chains, e.g. metamethod dispatch under a JIT'd op). With
/// nested entries restoring an outer Vm pointer becomes load-bearing.
///
/// J-D therefore turns the guard into a real RAII: `enter_jit`
/// captures the prior `(JIT_VM, JIT_CL)` values into the guard, and
/// `Drop` restores them. The cost is the 2 TLS writes we previously
/// elided per dispatch. The single-thread-no-nesting case is
/// semantically equivalent: prior values restored on exit are the
/// same null/stale ones the next `enter_jit` would overwrite anyway;
/// no observable behavior change for existing call sites.
///
/// `NullJitBackend::enter` keeps the no-op shape via
/// [`noop_jit_guard`] — its guard has `restore = None`.
///
/// Debug-mode assertions still catch misuse via `current_jit_vm`'s
/// is_null check IF a helper runs outside `enter_jit` (would happen
/// only if some bug calls a helper symbol from interp code, which
/// never happens by design).
#[must_use = "the guard must outlive the JIT entry call"]
pub struct JitVmGuard {
    /// `None` = inert (NullJitBackend's no-op). `Some` = real RAII:
    /// the dispatcher's prior `(JIT_VM, JIT_CL)` values are captured
    /// here on entry, and `restore_fn` is the luna-jit-side restorer
    /// that writes them back on drop.
    ///
    /// Field is `pub(crate)` for [`noop_jit_guard`] (this file); the
    /// other constructor (luna-jit's `enter_jit`) lives in a separate
    /// crate, so [`JitVmRebindRestore`] itself is the public seam.
    pub(crate) restore: Option<JitVmRebindRestore>,
}

impl JitVmGuard {
    /// Construct a guard from a captured restore record. Called only
    /// by luna-jit's `enter_jit`; embedders never call this directly.
    #[doc(hidden)]
    #[inline]
    pub fn from_restore(r: JitVmRebindRestore) -> Self {
        Self { restore: Some(r) }
    }
}

/// Capture of the previous `(JIT_VM, JIT_CL)` slot contents at the
/// moment [`JitVmGuard`] takes ownership, plus a function pointer
/// that writes them back. The function pointer lets luna-core hold
/// the drop semantics without referencing Cranelift's TLS storage
/// (the cells themselves live in `luna_jit::jit_backend` because
/// that's where the `thread_local!` block is, but the cells hold
/// luna-core types — `*mut Vm` and `*const LuaClosure`).
///
/// Fields are `pub` rather than `pub(crate)` because luna-jit's
/// `enter_jit` needs to construct an instance, and luna-jit is a
/// separate crate. The whole type is `#[doc(hidden)]` so it stays
/// out of the embedder surface.
#[doc(hidden)]
pub struct JitVmRebindRestore {
    pub prev_vm: *mut crate::vm::Vm,
    pub prev_cl: *const crate::runtime::LuaClosure,
    /// luna-jit-side function that writes `(prev_vm, prev_cl)` back
    /// into the TLS cells. Set by `enter_jit`; never null when this
    /// struct exists.
    pub restore_fn:
        unsafe fn(prev_vm: *mut crate::vm::Vm, prev_cl: *const crate::runtime::LuaClosure),
}

impl Drop for JitVmGuard {
    fn drop(&mut self) {
        if let Some(r) = self.restore.take() {
            // SAFETY: `restore_fn` is set only by luna-jit's
            // `enter_jit`, which threads the prior TLS values into
            // the guard at construction time. The fn ptr type is
            // `unsafe fn` because the cells participate in the JIT
            // helper SAFETY contract (`current_jit_vm` /
            // `current_jit_closure` rely on slot validity during the
            // dispatch window).
            unsafe { (r.restore_fn)(r.prev_vm, r.prev_cl) };
        }
    }
}
