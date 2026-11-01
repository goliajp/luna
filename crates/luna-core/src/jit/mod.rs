//! luna-core JIT surface — **trait + types only, zero Cranelift**.
//!
//! The interpreter dispatcher in [`crate::vm::exec`] routes JIT calls
//! through [`IntChunkCompiler`] / [`TraceCompiler`] trait objects;
//! luna-core ships only the no-op [`NullJitBackend`], which makes the
//! whole crate Cranelift-free (and therefore zero-three-party-dep).
//!
//! The Cranelift-backed implementations (`CraneliftBackend`, the 26
//! `luna_jit_*` extern "C" helpers, the `JIT_CACHE` thread-local,
//! `enter_jit`, `cache_lookup_or_compile`, `try_compile_int_chunk`,
//! `try_compile_trace_with_options`, …) live in the `luna` crate
//! under `luna::jit_backend`. Embedders who want the JIT use
//! `luna::Vm::new_minimal_with_jit(version)` instead of
//! `luna_core::vm::Vm::new_minimal(version)`.
//!
//! The interpreter runs a backend's code without checking it, so a
//! backend is an `unsafe impl`. Nothing safe changes what compiled code a
//! prototype or an installed trace holds, nor replaces or frees the
//! storage that code lives in (clearing a storage's cache leaves the code
//! mapped); taking a trace's tier-up source only keeps the trace at its
//! tier. The example compiles; each one after it fails to compile, for the
//! reason its comment gives.
//!
//! ```
//! use luna_core::{jit::*, runtime::{Gc, LuaClosure, Proto, Value}, version::LuaVersion, vm::Vm};
//! struct None_;
//! // SAFETY: it compiles nothing
//! unsafe impl IntChunkCompiler for None_ {
//!     fn try_compile(&self, _: &mut dyn luna_core::__private::JitStorage, _: Gc<Proto>, _: bool,
//!         _: bool) -> CompileResult { CompileResult::Skipped }
//!     fn enter(&self, _: *mut Vm, _: Option<Gc<LuaClosure>>) -> JitVmGuard { noop_jit_guard() }
//! }
//! let mut vm = Vm::new(LuaVersion::Lua54);
//! let Value::Closure(f) = vm.eval("return function() end").unwrap()[0] else { panic!() };
//! let _ = (f.proto.trace_hot_count(), f.proto.call_hot_count());
//! let _ = TCellPtr::null().get();
//! ```
//!
//! ```compile_fail,E0200
//! use luna_core::{jit::*, runtime::{Gc, LuaClosure, Proto}, vm::Vm};
//! struct None_;
//! impl IntChunkCompiler for None_ { // error[E0200]: the trait is `unsafe`
//!     fn try_compile(&self, _: &mut dyn luna_core::__private::JitStorage, _: Gc<Proto>, _: bool,
//!         _: bool) -> CompileResult { CompileResult::Skipped }
//!     fn enter(&self, _: *mut Vm, _: Option<Gc<LuaClosure>>) -> JitVmGuard { noop_jit_guard() }
//! }
//! ```
//!
//! ```compile_fail,E0616
//! use luna_core::{runtime::Value, version::LuaVersion, vm::Vm};
//! let mut vm = Vm::new(LuaVersion::Lua54);
//! let Value::Closure(f) = vm.eval("return function() end").unwrap()[0] else { panic!() };
//! let _ = f.proto.jit.get(); // error[E0616]: field `jit` is private
//! ```
//!
//! ```compile_fail,E0624
//! luna_core::jit::TCellPtr::null().set(std::ptr::null()); // error[E0624]: `set` is private
//! ```
//!
//! ```compile_fail,E0624
//! use luna_core::{runtime::Value, version::LuaVersion, vm::Vm};
//! let mut vm = Vm::new(LuaVersion::Lua54);
//! let Value::Closure(f) = vm.eval("return function() end").unwrap()[0] else { panic!() };
//! let _ = vm.collect_proto_hashes(f.proto);
//! # let t: luna_core::__private::CompiledTrace = unimplemented!();
//! vm.install_aot_trace(f.proto, t); // error[E0624]: `install_aot_trace` is private
//! ```

// Pure data types + small cranelift-free helpers for the trace JIT.
pub mod trace_types;
pub use trace_types::*;

// Wire format for AOT trace
// metadata (header + index entry + encode/decode). Pure data; both
// `luna-aot` (compile-time encode) and `luna-runtime-helpers`
// (deploy-time decode) depend on this single module so the on-disk
// shape stays in lock-step.
pub mod aot_meta;

// Backend trait surface. The Cranelift-bound CraneliftBackend lives in
// luna's jit_backend module.
pub(crate) mod abi;
pub(crate) mod rebind;
pub(crate) use abi::TraceCompiler;
pub use abi::{
    CompileResult, IntChunkCompiler, IntChunkFn, IntFn1, IntFn2, IntFn3, IntFn4, MAX_JIT_ARITY,
    NullJitBackend,
};
pub(crate) use rebind::JitVmRebindRestore;

// Per-Vm JIT storage trait. Holds the JIT cache + handle collections
// so a Vm carries its own JIT state across thread moves.
pub(crate) mod storage;
pub(crate) use storage::JitStorage;
pub use storage::NullJitStorage;

// cfg-gated Send-friendly aliases & wrappers for
// the trace IR interior-mutability types. Default-feature path is
// 0-cost (`Rc` / `Cell` / `RefCell`); `feature = "send"` flips to
// `Arc` / `AtomicU32` / `AtomicBool` / `AtomicPtr<u8>` / `RwLock` so
// `CompiledTrace` + `Proto::traces` become structurally Send + Sync.
pub mod send_compat;
pub use send_compat::{TArc, TCellBool, TCellPtr, TCellU32, TRefLock};

/// Makes code another thread wrote and published runnable on this thread.
/// Call it after reading, with acquire ordering, the entry the writer
/// published (after its instruction cache maintenance), and before running
/// the code. On aarch64 the core may have fetched instructions at that
/// address before they were written; `isb` discards what it fetched ahead.
/// The writer's cache maintenance reaches every core already, so this is
/// all the reader needs, on any operating system. x86 keeps instruction
/// fetch coherent with stores.
#[inline]
pub fn code_fence() {
    #[cfg(target_arch = "aarch64")]
    // SAFETY: `isb` flushes this core's pipeline only; it reads and writes
    // no memory and no register
    unsafe {
        std::arch::asm!("isb", options(nostack, preserves_flags))
    };
}

// Compatibility re-export so external `use luna::jit::trace::*` paths
// (and `crate::jit::trace::*` accesses inside this crate) keep
// resolving. In luna-core `trace` is just a namespace alias for `trace_types`; in
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

/// RAII guard pinning the active `Vm` (and optional closure)
/// pointer for JIT-emitted Rust helper calls.
///
/// # Real RAII rebind
///
/// `enter_jit` captures the prior `(JIT_VM, JIT_CL)` values into the
/// guard, and `Drop` restores them. A drop that skipped the restore
/// would save 2 TLS writes per dispatch, but under `feature = "send"`
/// a Vm can park on thread A, move to thread B, and dispatch there:
/// the per-thread `JIT_VM` slot on thread B is null/stale at entry,
/// and a no-op drop would leave stale Vm pointers behind for any
/// **nested** JIT entry (Lua-from-Rust-from-JIT call chains, e.g.
/// metamethod dispatch under a JIT'd op). With nested entries,
/// restoring the outer Vm pointer is load-bearing. In the
/// single-thread-no-nesting case the values restored on exit are the
/// same null/stale ones the next `enter_jit` would overwrite anyway.
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
    #[inline]
    pub(crate) fn from_restore(r: JitVmRebindRestore) -> Self {
        Self { restore: Some(r) }
    }
}

impl Drop for JitVmGuard {
    fn drop(&mut self) {
        if let Some(r) = self.restore.take() {
            (r.restore_fn)(r.prev_vm, r.prev_cl);
        }
    }
}
