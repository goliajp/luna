//! `Send` wrapper newtype for `cranelift_jit::JITModule`.
//!
//! ## Why this exists
//!
//! A per-Vm JIT cache on a cross-thread-sendable Vm requires
//! `JITModule` to be `Send`. As of cranelift_jit 0.124.3 the
//! type is **not** auto-`Send` because its
//! `memory: Box<dyn JITMemoryProvider>` field is a trait object with
//! no `+ Send` bound. The default provider — `SystemMemoryProvider` —
//! IS `Send`
//! (`cranelift-jit-0.124.3/src/memory/system.rs:126 unsafe impl Send
//! for Memory`), and luna never plugs in a custom provider
//! (`grep memory_provider crates/luna-jit/src/` → 0 hits), so
//! wrapping `JITModule` in a newtype + `unsafe impl Send` is sound
//! for luna's actual usage pattern.
//!
//! Precedent: `unsafe impl Send for TraceHandle` in `trace.rs`.

use cranelift_jit::JITModule;
use std::ops::{Deref, DerefMut};

/// `Send`-asserting newtype around [`cranelift_jit::JITModule`].
///
/// Wraps the module so it can live in a `Send` container (a per-`Vm`
/// field) without an inner trait-object Send bound from upstream
/// Cranelift.
///
/// **Not a stable embedder API.** The type is `pub` only so the
/// integration test (`tests/it/send_jit_module_wrapper.rs`) can
/// import it via a `#[doc(hidden)]` re-export at the crate root —
/// embedders should treat it as internal to luna-jit.
///
/// **Not `Sync`.** `JITModule` contains `RefCell<symbols>` (interior
/// mutability with non-atomic borrow tracking) so by-ref sharing
/// across threads is unsound; the Vm is move-only across threads and
/// uses an `RwLock` at the outer Vm level to gate mutator access.
#[doc(hidden)]
pub struct SendJitModule(JITModule);

// SAFETY: on cranelift_jit 0.124.3 the only `!Send` field is
// `memory: Box<dyn JITMemoryProvider>` (`backend.rs:175`, the trait object has no `+ Send` bound). The
// default concrete provider `SystemMemoryProvider` IS `Send`
// (`memory/system.rs:126 unsafe impl Send for Memory`). luna never
// calls `JITBuilder::memory_provider` (`grep memory_provider
// crates/luna-jit/src/` → 0 hits), so every `JITModule` luna
// constructs holds the default `SystemMemoryProvider`. Mirrors the
// established precedent `unsafe impl Send for TraceHandle` in
// `trace.rs`.
//
// Caveat: future cranelift bumps must re-check this; the
// static assertion in `tests/it/send_jit_module_wrapper.rs` is the
// canary.
unsafe impl Send for SendJitModule {}

impl SendJitModule {
    /// Wraps a freshly-built `JITModule`. Caller MUST have used the
    /// default memory provider path (`JITBuilder::new` /
    /// `JITBuilder::with_isa` without `memory_provider(...)`); see
    /// SAFETY note above.
    #[inline]
    #[allow(dead_code)]
    pub fn new(module: JITModule) -> Self {
        Self(module)
    }

    /// Borrows the wrapped module immutably.
    #[allow(dead_code)]
    #[inline]
    pub fn get(&self) -> &JITModule {
        &self.0
    }

    /// Borrows the wrapped module mutably.
    #[allow(dead_code)]
    #[inline]
    pub fn get_mut(&mut self) -> &mut JITModule {
        &mut self.0
    }

    /// Unwraps the inner module. Loses the `Send` marker once
    /// extracted; caller becomes responsible for re-wrapping if it
    /// must cross threads again.
    #[allow(dead_code)] // leave for ergonomics
    #[inline]
    pub fn into_inner(self) -> JITModule {
        self.0
    }
}

impl SendJitModule {
    /// Frees the module's code and data.
    ///
    /// # Safety
    ///
    /// No function of the module is running or will be called again.
    pub(crate) unsafe fn free(self) {
        // SAFETY: forwarded from the caller
        unsafe { self.0.free_memory() }
    }
}

/// A module being compiled: no pointer into its code has been handed out
/// yet, so dropping it (a compile that bails) frees the code.
/// [`Self::publish`] hands it over once entry points leave it.
pub(crate) struct UnpublishedModule(Option<JITModule>);

impl UnpublishedModule {
    pub(crate) fn new(module: JITModule) -> Self {
        Self(Some(module))
    }

    pub(crate) fn publish(mut self) -> SendJitModule {
        SendJitModule(self.0.take().expect("published once"))
    }
}

impl Drop for UnpublishedModule {
    fn drop(&mut self) {
        if let Some(module) = self.0.take() {
            // SAFETY: nothing outside this value has seen its code
            unsafe { module.free_memory() }
        }
    }
}

impl Deref for UnpublishedModule {
    type Target = JITModule;

    fn deref(&self) -> &JITModule {
        self.0.as_ref().expect("published once")
    }
}

impl DerefMut for UnpublishedModule {
    fn deref_mut(&mut self) -> &mut JITModule {
        self.0.as_mut().expect("published once")
    }
}

impl Deref for SendJitModule {
    type Target = JITModule;

    #[inline]
    fn deref(&self) -> &JITModule {
        &self.0
    }
}

impl DerefMut for SendJitModule {
    #[inline]
    fn deref_mut(&mut self) -> &mut JITModule {
        &mut self.0
    }
}
