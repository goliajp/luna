//! Per-`Vm` JIT storage trait + null impl.
//!
//! The Cranelift JIT keeps three thread-local collections that own
//! mmap'd code pages for compiled chunk fns and compiled traces:
//!
//! - `JIT_CACHE` — hash map of bytecode-key → cached compile result
//! - `JIT_CACHE_HANDLES` — `Vec<JitHandle>` holding each compiled
//!   chunk's `JITModule` so the entry pointer stays callable
//! - `TRACE_JIT_HANDLES` — `Vec<TraceHandle>` holding each compiled
//!   trace's `JITModule`
//!
//! These live in per-`Vm` field storage rather than `thread_local!`.
//! The Cranelift types (`JITModule`, `CacheEntry`,
//! `JitHandle`, `TraceHandle`) live in luna-jit, so luna-core only
//! sees an opaque [`JitStorage`] trait + a no-op
//! [`NullJitStorage`] default; the concrete `CraneliftJitStorage`
//! impl lives in `luna_jit::jit_backend::storage`.

/// Per-`Vm` JIT storage. Held as `Box<dyn JitStorage>` on
/// [`crate::vm::jit_state::JitState::storage`]. The concrete impl is
/// chosen by whoever installs the JIT backend (luna-core's default
/// is [`NullJitStorage`]; the `luna_jit` crate swaps in its
/// `CraneliftJitStorage` via a setter alongside `install_jit_backend`).
///
/// luna-core treats the trait as opaque — readers downcast through
/// [`std::any::Any`] to reach concrete fields. This keeps the
/// `JITModule`-bearing types (and therefore the Cranelift dep) out
/// of luna-core.
///
/// # Safety
/// The Vm calls [`Self::clear_cache`] from safe code
/// ([`VmJitState::jit_clear_cache`](crate::__private::VmJitState::jit_clear_cache)):
/// it must leave mapped every piece of code an entry handed out through
/// this storage points into, so that no entry a prototype or an installed
/// trace holds stops being live code. Code is unmapped only by
/// [`Self::release_code`] or by dropping the storage. The example
/// compiles, the one after it fails to.
///
/// ```
/// struct Empty;
/// // SAFETY: it holds no code
/// unsafe impl luna_core::__private::JitStorage for Empty {
///     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
///     fn as_any(&self) -> &dyn std::any::Any { self }
/// }
/// ```
///
/// ```compile_fail,E0200
/// struct Empty;
/// impl luna_core::__private::JitStorage for Empty { // error[E0200]: the trait is `unsafe`
///     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
///     fn as_any(&self) -> &dyn std::any::Any { self }
/// }
/// ```
pub unsafe trait JitStorage: std::any::Any {
    /// Mutable downcast hook. luna-jit's `CraneliftBackend`
    /// implementations call this then `downcast_mut::<CraneliftJitStorage>()`
    /// to reach the concrete cache + handle collections.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;

    /// Immutable downcast hook. Symmetric with [`Self::as_any_mut`];
    /// used by read-only diagnostics.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Forgets the compiled code kept for reuse by cache key, so the next
    /// compile of the same function compiles it again. The code itself
    /// stays mapped (the trait's `# Safety`): prototypes and traces that
    /// hold its entries keep running it until [`Self::release_code`].
    fn clear_cache(&mut self) {}

    /// Called by the `Vm` identified by `vm` before it asks its compilers
    /// for code through this storage. Code a storage holds can be
    /// released only if every compile through it came from one `Vm`.
    fn claim(&mut self, vm: u64) {
        let _ = vm;
    }

    /// Frees the machine code this storage holds, provided every compile
    /// through it was claimed by `vm`; otherwise keeps it.
    ///
    /// # Safety
    ///
    /// `vm` is going away: none of its code is running and none will be
    /// entered again. The `Vm` calls this from its `Drop`.
    unsafe fn release_code(&mut self, vm: u64) {
        let _ = vm;
    }
}

/// No-op storage installed by [`crate::vm::Vm::new_minimal`]. Holds
/// nothing; downcasting from luna-jit will fail by design (a
/// `NullJitBackend` is paired with `NullJitStorage` — neither
/// `try_compile` nor `try_compile_trace` reaches the downcast site
/// because both immediately return `Skipped` / `None`).
#[derive(Default)]
pub struct NullJitStorage;

// SAFETY: it holds no code
unsafe impl JitStorage for NullJitStorage {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
