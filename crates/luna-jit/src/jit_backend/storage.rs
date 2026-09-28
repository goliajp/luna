//! Concrete per-`Vm` Cranelift JIT storage.
//!
//! Holds the cache + compiled-handle collections. Installed on a `Vm`
//! by `crate::install_default_jit` alongside
//! `install_jit_backend(CraneliftBackend, CraneliftBackend)`.
//!
//! Type-erased through [`luna_core::jit::JitStorage`] so luna-core
//! never needs to see `cranelift_jit::JITModule` (preserves the 0
//! third-party dep gate on luna-core).

use super::trace::TraceHandle;
use super::{CacheEntry, JitHandle};
use luna_core::jit::JitStorage;

/// Per-`Vm` Cranelift JIT storage. Three collections:
///
/// - `cache`: bytecode-keyed `HashMap<u64, CacheEntry>`.
/// - `cache_handles`: `Vec<JitHandle>` owning each compiled chunk's
///   `JITModule`.
/// - `trace_handles`: `Vec<TraceHandle>` owning each compiled
///   trace's `JITModule`.
///
/// The handle collections are append-only. The code is freed when the
/// `Vm` that compiled all of it drops ([`JitStorage::release_code`]);
/// dropping the storage alone leaves it mapped, since functions of some
/// `Vm` may still point into it.
#[derive(Default)]
pub(crate) struct CraneliftJitStorage {
    pub(crate) cache: std::collections::HashMap<u64, CacheEntry>,
    pub(crate) cache_handles: Vec<JitHandle>,
    pub(crate) trace_handles: Vec<TraceHandle>,
    /// The `Vm` that compiles through this storage.
    owner: Option<u64>,
    /// A second `Vm` compiled through it too: its code is never freed.
    shared: bool,
}

impl JitStorage for CraneliftJitStorage {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn claim(&mut self, vm: u64) {
        match self.owner {
            None => self.owner = Some(vm),
            Some(o) if o != vm => self.shared = true,
            Some(_) => {}
        }
    }

    unsafe fn release_code(&mut self, vm: u64) {
        if self.shared || self.owner != Some(vm) {
            return;
        }
        self.cache.clear();
        for h in self.cache_handles.drain(..) {
            // SAFETY: only `vm` compiled through this storage, and it is
            // going away (the caller's contract)
            unsafe { h.free() }
        }
        for h in self.trace_handles.drain(..) {
            // SAFETY: as above
            unsafe { h.free() }
        }
    }
}

/// Error returned by [`from_storage`] when the `Vm.jit.storage` slot
/// holds a [`JitStorage`] impl other than [`CraneliftJitStorage`].
///
/// This is a `Result` rather than a panic because the JIT compile
/// path can run under a C-ABI callback (any of the `luaL_*` / `lua_*`
/// entrypoints in [`crate::capi`]), and a Rust panic across the
/// `extern "C"` boundary triggers `fatal runtime error: failed to initiate panic` and
/// aborts the process with SIGABRT — panic-into-`extern "C"` is UB
/// and the runtime aborts rather than unwind.
///
/// Returning `Result` lets callers (the four `from_storage` call
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
/// and is the recommended entrypoint; `luaL_newstate` uses it. Hand-rolled embedders that install only one half observe
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
