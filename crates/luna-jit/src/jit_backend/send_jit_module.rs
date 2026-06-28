//! v2.0 Track J sub-step J-A — `Send` wrapper newtype for
//! `cranelift_jit::JITModule`.
//!
//! ## Why this exists
//!
//! Track J's Option B (per-Vm JIT cache, Vm cross-thread sendable)
//! requires `JITModule` to be `Send`. As of cranelift_jit 0.124.3 the
//! type is **not** auto-`Send` because its
//! `memory: Box<dyn JITMemoryProvider>` field is a trait object with
//! no `+ Send` bound (see audit:
//! `.dev/rfcs/v2.0-track-j-prep.md` §Sub 3). The default concrete
//! provider — `SystemMemoryProvider` — IS `Send`
//! (`cranelift-jit-0.124.3/src/memory/system.rs:126 unsafe impl Send
//! for Memory`), and luna never plugs in a custom provider
//! (`grep memory_provider crates/luna-jit/src/` → 0 hits), so
//! wrapping `JITModule` in a newtype + `unsafe impl Send` is sound
//! for luna's actual usage pattern.
//!
//! This sub-step (J-A) only lands the wrapper + smoke test. The
//! field migration of `JIT_CACHE` / `JIT_CACHE_HANDLES` /
//! `TRACE_JIT_HANDLES` from `thread_local!` to `Vm.VmJitStorage` is
//! J-B's job; J-A keeps existing call sites unchanged.
//!
//! Precedent: `unsafe impl Send for TraceHandle` at
//! `trace.rs:2497`.

use cranelift_jit::JITModule;
use std::ops::{Deref, DerefMut};

/// `Send`-asserting newtype around [`cranelift_jit::JITModule`].
///
/// Wraps the module so it can live in a `Send` container (per-`Vm`
/// field under Track J Option B) without an inner trait-object Send
/// bound from upstream Cranelift.
///
/// **Not a stable embedder API.** The type is `pub` only so the J-A
/// integration test (`tests/j_a_send_jit_module_wrapper.rs`) can
/// import it via a `#[doc(hidden)]` re-export at the crate root —
/// embedders should treat it as internal to luna-jit. J-B field
/// migration / J-D rebind consume it; nothing else.
///
/// **Not `Sync`.** `JITModule` contains `RefCell<symbols>` (interior
/// mutability with non-atomic borrow tracking) so by-ref sharing
/// across threads is unsound; Track J Option B is move-only and
/// uses an `RwLock` at the outer Vm level to gate mutator access.
#[doc(hidden)]
pub struct SendJitModule(JITModule);

// SAFETY: J-A audit on cranelift_jit 0.124.3 confirmed (per
// `.dev/rfcs/v2.0-track-j-prep.md` §Sub 3-4) that `JITModule`'s sole
// `!Send` field is `memory: Box<dyn JITMemoryProvider>`
// (`backend.rs:175`, the trait object has no `+ Send` bound). The
// default concrete provider `SystemMemoryProvider` IS `Send`
// (`memory/system.rs:126 unsafe impl Send for Memory`). luna never
// calls `JITBuilder::memory_provider` (`grep memory_provider
// crates/luna-jit/src/` → 0 hits), so every `JITModule` luna
// constructs holds the default `SystemMemoryProvider`. Mirrors the
// established precedent `unsafe impl Send for TraceHandle` at
// `trace.rs:2497`.
//
// Caveat: future cranelift bumps must re-run the
// `.dev/rfcs/v2.0-track-j-prep.md` §Sub 3 field-by-field table; the
// static assertion in `tests/j_a_send_jit_module_wrapper.rs` is the
// canary.
unsafe impl Send for SendJitModule {}

impl SendJitModule {
    /// Wraps a freshly-built `JITModule`. Caller MUST have used the
    /// default memory provider path (`JITBuilder::new` /
    /// `JITBuilder::with_isa` without `memory_provider(...)`); see
    /// SAFETY note above.
    #[inline]
    #[allow(dead_code)] // J-B will consume this.
    pub fn new(module: JITModule) -> Self {
        Self(module)
    }

    /// Borrows the wrapped module immutably.
    #[allow(dead_code)] // J-B will consume this.
    #[inline]
    pub fn get(&self) -> &JITModule {
        &self.0
    }

    /// Borrows the wrapped module mutably.
    #[allow(dead_code)] // J-B will consume this.
    #[inline]
    pub fn get_mut(&mut self) -> &mut JITModule {
        &mut self.0
    }

    /// Unwraps the inner module. Loses the `Send` marker once
    /// extracted; caller becomes responsible for re-wrapping if it
    /// must cross threads again.
    #[allow(dead_code)] // J-B / J-D may consume; leave for ergonomics.
    #[inline]
    pub fn into_inner(self) -> JITModule {
        self.0
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
