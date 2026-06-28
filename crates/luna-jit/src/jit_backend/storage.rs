//! v2.0 Track J sub-step J-B — concrete per-`Vm` Cranelift JIT
//! storage.
//!
//! Holds the cache + compiled-handle collections that used to live in
//! `thread_local!`s on `luna_jit::jit_backend::{mod,trace}`. Installed
//! on a `Vm` by `crate::install_default_jit` alongside
//! `install_jit_backend(CraneliftBackend, CraneliftBackend)`.
//!
//! Phases C/D/E/F migrate the TLS sites onto this struct one collection
//! at a time. Phase C lands the struct with empty placeholder fields —
//! the TLS reads/writes still go through `thread_local!` storage on
//! this commit, and the per-`Vm` fields stay unread until their
//! respective TLS site is rewired.
//!
//! Type-erased through [`luna_core::jit::JitStorage`] so luna-core
//! never needs to see `cranelift_jit::JITModule` (preserves the 0
//! third-party dep gate on luna-core).
//!
//! Single-thread semantics preserved at this step; cross-thread Send
//! transfer is J-D / J-E.

use super::trace::TraceHandle;
use super::{CacheEntry, JitHandle};
use luna_core::jit::JitStorage;

/// Per-`Vm` Cranelift JIT storage. Three collections:
///
/// - `cache`: bytecode-keyed `HashMap<u64, CacheEntry>` (Phase D
///   target — replaces `JIT_CACHE` TLS).
/// - `cache_handles`: `Vec<JitHandle>` owning each compiled chunk's
///   `JITModule` (Phase E target — replaces `JIT_CACHE_HANDLES` TLS).
/// - `trace_handles`: `Vec<TraceHandle>` owning each compiled
///   trace's `JITModule` (Phase F target — replaces
///   `TRACE_JIT_HANDLES` TLS).
///
/// Each `Vec` / `HashMap` is append-only / insert-only for the life
/// of the `Vm`; dropping the storage releases the underlying mmap
/// pages.
#[derive(Default)]
pub(crate) struct CraneliftJitStorage {
    pub(crate) cache: std::collections::HashMap<u64, CacheEntry>,
    pub(crate) cache_handles: Vec<JitHandle>,
    pub(crate) trace_handles: Vec<TraceHandle>,
}

impl JitStorage for CraneliftJitStorage {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Error returned by [`from_storage`] when the `Vm.jit.storage` slot
/// holds a [`JitStorage`] impl other than [`CraneliftJitStorage`].
///
/// v2.0 J-B follow-up — the original `from_storage` did an `expect`
/// panic on mismatch. When the JIT compile path runs under a C-ABI
/// callback (any of the `luaL_*` / `lua_*` entrypoints in
/// [`crate::capi`]), a Rust panic across the `extern "C"` boundary
/// triggers `fatal runtime error: failed to initiate panic` and
/// aborts the process with SIGABRT — panic-into-`extern "C"` is UB
/// and the runtime aborts rather than unwind.
///
/// Converting to `Result` lets callers (the four `from_storage` call
/// sites in [`crate::jit_backend`] and [`crate::jit_backend::trace`])
/// observe the mismatch and degrade to "no JIT" (`CompileResult::Skipped`
/// / `None`), which the dispatcher already handles as the normal
/// "this Proto stays on interp" path. The C-ABI boundary therefore
/// completes the call via interp instead of aborting.
///
/// This is graceful-degradation, not a silent error: the underlying
/// misconfig is `install_jit_backend(Cranelift, Cranelift)` without
/// the paired `install_jit_storage(CraneliftJitStorage)`. The
/// `crate::install_default_jit` shim installs both halves atomically
/// and is the recommended entrypoint; `luaL_newstate` was updated to
/// use it. Hand-rolled embedders that install only one half observe
/// "JIT silently disabled" rather than process abort.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StorageMismatch;

/// Downcast helper. Returns `Err(StorageMismatch)` if the Vm's storage
/// isn't a `CraneliftJitStorage` — see [`StorageMismatch`] for why.
#[inline]
pub(crate) fn from_storage(
    storage: &mut dyn JitStorage,
) -> Result<&mut CraneliftJitStorage, StorageMismatch> {
    storage
        .as_any_mut()
        .downcast_mut::<CraneliftJitStorage>()
        .ok_or(StorageMismatch)
}
