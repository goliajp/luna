//! v2.0 Track J sub-step J-B — per-`Vm` JIT storage trait + null impl.
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
//! J-B moves these three from `thread_local!` to per-`Vm` field
//! storage. The Cranelift types (`JITModule`, `CacheEntry`,
//! `JitHandle`, `TraceHandle`) live in luna-jit, so luna-core only
//! sees an opaque [`JitStorage`] trait + a no-op
//! [`NullJitStorage`] default; the concrete `CraneliftJitStorage`
//! impl lives in `luna_jit::jit_backend::storage`.
//!
//! See `.dev/rfcs/v2.0-track-j-b-design.md` for the migration design
//! (integration pattern, soundness preservation, phase plan).
//!
//! Single-thread semantics are preserved by this sub-step; cross-
//! thread Send transfer is J-D / J-E's lift.

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
pub trait JitStorage: std::any::Any {
    /// Mutable downcast hook. luna-jit's `CraneliftBackend`
    /// implementations call this then `downcast_mut::<CraneliftJitStorage>()`
    /// to reach the concrete cache + handle collections.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;

    /// Immutable downcast hook. Symmetric with [`Self::as_any_mut`];
    /// used by read-only diagnostics.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// No-op storage installed by [`crate::vm::Vm::new_minimal`]. Holds
/// nothing; downcasting from luna-jit will fail by design (a
/// `NullJitBackend` is paired with `NullJitStorage` — neither
/// `try_compile` nor `try_compile_trace` reaches the downcast site
/// because both immediately return `Skipped` / `None`).
#[derive(Default)]
pub struct NullJitStorage;

impl JitStorage for NullJitStorage {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
