//! Owner of a per-`Vm` `cranelift_jit::JITModule`.
//!
//! `JITModule` is `Send` on its own; the newtype is the one type that
//! owns published code and frees it.

use cranelift_jit::JITModule;
use std::ops::{Deref, DerefMut};

/// Newtype around [`cranelift_jit::JITModule`] that owns a `Vm`'s
/// published code.
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

impl SendJitModule {
    /// Wraps a freshly-built `JITModule`.
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

    /// Unwraps the inner module.
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
