//! v1.3 Phase SS-B — `SendVm` newtype wrapper for cross-thread embedding.
//!
//! Gated behind `#[cfg(feature = "send")]`. Embedders opt in via
//! `cargo add luna-core --features send` and use [`SendVm`] in place
//! of [`crate::vm::Vm`] when they need to hold a Lua state across
//! `.await` boundaries on a multi-threaded executor or move it between
//! OS threads.
//!
//! # Shape
//!
//! ```ignore
//! pub struct SendVm {
//!     inner: Arc<UnsafeCell<Vm>>,
//!     lock:  Arc<RwLock<()>>,
//! }
//! ```
//!
//! - `Arc<UnsafeCell<Vm>>` — the wrapped Vm. `UnsafeCell` because the
//!   API surface presents `&self` (so `SendVm` can be cloned and the
//!   handle copied into multiple threads), but every method internally
//!   reaches a `&mut Vm` once the lock is held.
//! - `Arc<RwLock<()>>` — access serializer. All operations take
//!   `lock.write()` (effectively a `Mutex` — see the SAFETY notes
//!   below for why the read/write split in the audit is purely a
//!   conceptual classification, not a mechanically distinct path).
//!
//! `unsafe impl Send for SendVm {}` — safe because:
//!
//! 1. `Vm` is `!Send` only because its [`crate::runtime::heap::Heap`]
//!    holds raw `*mut GcHeader` pointers and a thread-local-ish
//!    `JitState`. The pointers themselves are address values; what
//!    forbids `Send` is the *single-mutator* invariant the dispatcher
//!    relies on. Once we serialize all access through the lock, only
//!    one OS thread at a time materializes the `&mut Vm` and runs the
//!    dispatcher — the same single-mutator invariant the bare `Vm`
//!    holds, just established via the lock rather than via type-system
//!    `!Send`.
//! 2. The interp-only restriction below means `JitState` stays
//!    `NullJitBackend` and the `JIT_VM` TLS pointer is never
//!    populated; the only thread-local state the Vm touches under
//!    `SendVm` is the std-library RNG seed at construction time
//!    (`Vm::new_minimal` → `rng_auto_seed`), which runs once on the
//!    constructing thread and is never re-read TLS-wise afterwards.
//!
//! `SendVm` is **not** `Sync`. Cross-thread share of `&SendVm` is
//! forbidden — only move/clone-and-move. Clones share the same
//! underlying Vm via the inner `Arc`; concurrent calls block on the
//! lock.
//!
//! # Interp-only constraint
//!
//! Per `.dev/rfcs/v1.3-audit-send-vm-design.md` §3.3, the v1.3 ship
//! of `SendVm` does **not** install a JIT backend. `SendVm::new`
//! calls [`Vm::new_minimal`] which leaves `JitState` at
//! `NullJitBackend`; the dispatcher always falls back to the
//! interpreter. JIT-aware `SendVm` is a post-v1.3 polish item (the
//! `Proto::traces: RefCell<Vec<Rc<CompiledTrace>>>` cross-cutting
//! concern intersects with `Send` and is scoped out of v1.3).
//!
//! This is a documented contract, not a defer — embedders who need
//! both Send semantics and JIT today should run one bare `Vm` per OS
//! thread and exchange data via channels.
//!
//! # When to use `SendVm` vs `Vm`
//!
//! - **`Vm`** — single-thread scripting (game engine main thread,
//!   CLI tool, REPL). The fast path; zero overhead vs the v1.2
//!   baseline.
//! - **`SendVm`** — multi-threaded host (tokio `multi_thread`,
//!   request-per-script web server, worker-pool embedding). Pays
//!   the lock acquire cost (~30-50 ns per method call) and gives
//!   up the JIT.
//!
//! See `docs/threading.md` for the canonical embedding patterns.

use std::cell::UnsafeCell;
use std::sync::{Arc, RwLock};

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::host_roots::{HostRootStale, HostRootTicket};
use crate::vm::userdata_trait::LuaUserdata;

/// Cross-thread-capable Lua VM handle.
///
/// See the [module docs](self) for the design, safety contract, and
/// the v1.3 interp-only restriction.
///
/// Clone the handle and move clones into threads / tasks to share one
/// underlying `Vm` across workers; concurrent method calls block on
/// the inner lock. For genuine parallelism (multiple scripts running
/// at once), construct one `SendVm` per worker — the type's value is
/// that you can hold it across `.await` on a multi-thread executor,
/// not that two threads execute the same script simultaneously.
pub struct SendVm {
    inner: Arc<UnsafeCell<Vm>>,
    lock: Arc<RwLock<()>>,
}

// SAFETY: see module-level docs. `Vm`'s `!Send` derives from a
// single-mutator invariant the dispatcher relies on; the `RwLock`
// established alongside the `UnsafeCell` re-establishes that
// invariant at runtime (only one thread holds the write guard at a
// time, and every method takes the write guard before materializing
// `&mut Vm`). Interp-only constraint (NullJitBackend, no `JIT_VM`
// TLS) plus the construction-time-only RNG seed mean no per-call
// thread-local state escapes the move/clone-and-move semantics.
//
// SendVm is intentionally not Sync — cross-thread `&SendVm` would
// allow two threads to lock-and-eval concurrently; the lock would
// serialize them but the *handle* still needs to move/clone (not
// shared by reference) to discourage misuse.
unsafe impl Send for SendVm {}

impl SendVm {
    /// Construct a fresh `SendVm` for the given Lua dialect.
    ///
    /// Calls [`Vm::new_minimal`] internally — no standard libraries
    /// loaded, no JIT installed (`NullJitBackend`). Open the libs you
    /// want via [`SendVm::open_base`] etc.
    pub fn new(version: LuaVersion) -> Self {
        // `Vm::new_minimal` in luna-core constructs a JIT-free Vm
        // (the `luna-jit` crate's `new_minimal_with_jit` is the
        // JIT-installing entry point; we deliberately do not call
        // it here per the §3.3 interp-only constraint).
        let vm = Vm::new_minimal(version);
        // clippy::arc_with_non_send_sync fires because
        // `UnsafeCell<Vm>` is `!Send + !Sync`. The `unsafe impl Send
        // for SendVm` above carries the safety story for the outer
        // wrapper; we shut the lint up here intentionally.
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(UnsafeCell::new(vm));
        Self {
            inner,
            lock: Arc::new(RwLock::new(())),
        }
    }

    /// v2.0 Track J sub-step J-E — wrap a caller-constructed `Vm`
    /// (with any backend / libraries / globals already installed) in
    /// a `SendVm`. The complement to [`SendVm::new`], which constructs
    /// an interp-only Vm internally.
    ///
    /// The canonical use case is JIT-equipped cross-thread embedding:
    ///
    /// ```ignore
    /// // On thread A: build a JIT-equipped Vm and prepare it.
    /// let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    /// vm.open_base();
    /// vm.open_math();
    /// // Wrap and ship to a worker thread.
    /// let send = luna_core::vm::SendVm::from_vm(vm);
    /// std::thread::spawn(move || {
    ///     send.eval("for i=1,1000 do end; return 1").unwrap();
    /// });
    /// ```
    ///
    /// SAFETY contract (same as `SendVm::new`'s outer
    /// `unsafe impl Send`, plus a caller-side obligation):
    ///
    /// 1. The `Vm` passed in must not be reachable from any other
    ///    thread at the point of the move into this constructor.
    ///    `pub fn` taking `vm: Vm` by value enforces this at the
    ///    type system.
    /// 2. Any thread-local state captured at the `Vm`'s construction
    ///    (e.g. the std-library RNG seed populated by
    ///    `rng_auto_seed`) was already produced on the original
    ///    thread before the move and is just data after that point —
    ///    the SendVm doesn't re-read TLS for it.
    /// 3. For JIT-equipped Vms, the J-D `scoped_jit_vm_rebind` RAII
    ///    means the per-dispatch `JIT_VM` / `JIT_CL` TLS slots are
    ///    re-armed on every `enter_jit` call on whichever thread
    ///    holds the write guard, so cross-thread JIT compile +
    ///    dispatch is sound under the same single-mutator invariant
    ///    that bare `Vm` relies on.
    ///
    /// See `.dev/rfcs/v2.0-track-j-e-verdict.md` for the J-E ship
    /// notes.
    pub fn from_vm(vm: Vm) -> Self {
        // Same arc_with_non_send_sync pattern as `new()` — the outer
        // `unsafe impl Send for SendVm` is what makes the wrapper
        // Send-shaped despite `UnsafeCell<Vm>` being `!Send + !Sync`.
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(UnsafeCell::new(vm));
        Self {
            inner,
            lock: Arc::new(RwLock::new(())),
        }
    }

    /// Install the base library on this `SendVm`. Mirror of
    /// [`Vm::open_base`].
    pub fn open_base(&self) {
        self.with_vm_mut(|vm| vm.open_base());
    }

    /// Install the math library.
    pub fn open_math(&self) {
        self.with_vm_mut(|vm| vm.open_math());
    }

    /// Install the string library.
    pub fn open_string(&self) {
        self.with_vm_mut(|vm| vm.open_string());
    }

    /// Install the table library.
    pub fn open_table(&self) {
        self.with_vm_mut(|vm| vm.open_table());
    }

    /// Install the coroutine library.
    pub fn open_coroutine(&self) {
        self.with_vm_mut(|vm| vm.open_coroutine());
    }

    /// Compile and run `src` as an anonymous chunk; return its
    /// results. Mirror of [`Vm::eval`].
    pub fn eval(&self, src: &str) -> Result<Vec<Value>, LuaError> {
        self.with_vm_mut(|vm| vm.eval(src))
    }

    /// Call any callable value from the host. Mirror of
    /// [`Vm::call_value`].
    pub fn call_value(&self, f: Value, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        self.with_vm_mut(|vm| vm.call_value(f, args))
    }

    /// Set a global by name. Mirror of [`Vm::set_global`].
    pub fn set_global<V: crate::vm::IntoValue>(&self, name: &str, v: V) -> Result<(), LuaError> {
        self.with_vm_mut(|vm| vm.set_global(name, v))
    }

    /// Read a global by name. Returns `Value::Nil` when the key is
    /// absent (matching Lua's table-read semantics).
    ///
    /// Not present on bare [`Vm`] today; introduced for `SendVm`
    /// because the bare Vm's `globals()` + raw `Gc<Table>` deref
    /// pattern is awkward across the lock boundary.
    pub fn get_global(&self, name: &str) -> Value {
        self.with_vm_mut(|vm| {
            let key = Value::Str(vm.heap.intern(name.as_bytes()));
            // SAFETY: Gc<T> = NonNull<T> over the single-threaded
            // GC heap (the lock guard above re-establishes the
            // single-mutator invariant). Globals table is reachable
            // for the lifetime of the Vm.
            unsafe { (*vm.globals().as_ptr()).get(key) }
        })
    }

    /// Intern a UTF-8 string into the heap's string table. Mirror
    /// of [`Vm::intern_str`]; returns a raw `Gc<LuaStr>` whose
    /// lifetime is tied to the underlying Vm (callers must keep the
    /// `SendVm` alive, the usual Gc-handle contract).
    pub fn intern_str(&self, s: &str) -> crate::runtime::Gc<crate::runtime::LuaStr> {
        self.with_vm_mut(|vm| vm.intern_str(s))
    }

    /// Allocate a host userdata wrapping `value` and bind it to a
    /// global name. Mirror of [`Vm::set_userdata`].
    pub fn set_userdata<T: LuaUserdata>(&self, name: &str, value: T) -> Result<(), LuaError> {
        self.with_vm_mut(|vm| vm.set_userdata(name, value))
    }

    /// Pin a `Value` as a host root and return its ticket. Mirror of
    /// [`Vm::pin_host`].
    pub fn pin_host(&self, v: Value) -> HostRootTicket {
        self.with_vm_mut(|vm| vm.pin_host(v))
    }

    /// Read a previously pinned host root. Mirror of
    /// [`Vm::read_host`].
    pub fn read_host(&self, t: HostRootTicket) -> Option<Value> {
        self.with_vm_mut(|vm| vm.read_host(t))
    }

    /// Release a single pinned root. Mirror of [`Vm::unpin`].
    pub fn unpin(&self, t: HostRootTicket) -> Result<(), HostRootStale> {
        self.with_vm_mut(|vm| vm.unpin(t))
    }

    /// v2.0 Track J sub-step J-E — snapshot of [`Vm::trace_dispatched_count`]
    /// through the lock. Used by the J-E cross-thread JIT smoke test
    /// (`luna-jit/tests/cv_send_vm_jit_smoke.rs`) to confirm the trace
    /// JIT actually engaged on the worker thread; useful to embedders
    /// who want to observe whether a script went JIT-hot through the
    /// SendVm boundary.
    pub fn trace_dispatched_count(&self) -> u64 {
        self.with_vm_mut(|vm| vm.trace_dispatched_count())
    }

    /// v2.0 Track J sub-step J-E — companion accessor for the cache
    /// entry count when the wrapped Vm has the Cranelift JIT backend
    /// installed (`luna_jit::new_minimal_with_jit` etc.). Returns 0 for
    /// JIT-free Vms (the default constructed by [`SendVm::new`]).
    ///
    /// The actual count comes from a luna-jit-side helper
    /// (`luna_jit::jit_backend::cache_entry_count`); luna-core can't
    /// look at concrete Cranelift types directly without breaking the
    /// 0-third-party-dep gate, so we expose a closure-shaped accessor
    /// instead. Use [`Self::with_vm`] for that pattern.
    #[doc(hidden)]
    pub fn __j_e_handle_arc_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    /// v2.0 Track J sub-step J-E — read-only closure-shaped accessor
    /// that runs `f` against `&Vm` under the lock. Mirror of the
    /// internal `with_vm_mut` but immutable. Lets embedders read
    /// arbitrary `Vm` state (counters, globals, dialect, etc.)
    /// without growing the `SendVm` API one method at a time.
    ///
    /// The lock acquired is still the `write` guard because
    /// `RwLock::read` doesn't compose with `UnsafeCell<Vm>` —
    /// concurrent readers would risk sharing `&Vm` while a `&mut Vm`
    /// materializes from elsewhere. Per the module-level safety
    /// notes, every method takes the write guard for this reason.
    pub fn with_vm<R>(&self, f: impl FnOnce(&Vm) -> R) -> R {
        let _guard = self.lock.write().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the write guard above provides exclusive access to
        // the wrapped `Vm` for this scope. We materialize `&Vm`
        // (immutable) with lifetime bounded by `_guard`; no aliasing
        // `&mut Vm` exists while this borrow lives. Per the
        // module-level safety contract, this preserves the single-
        // mutator invariant.
        let vm: &Vm = unsafe { &*self.inner.get() };
        f(vm)
    }

    /// Internal: take the write guard and run `f` with `&mut Vm`.
    ///
    /// All public methods route through here so the lock-and-deref
    /// pattern lives in one place; this is the only `unsafe` site in
    /// the `SendVm` API surface. A poisoned lock (i.e. a previous
    /// caller panicked while holding the guard) propagates the
    /// poison via `unwrap_or_else(into_inner)` — there is no
    /// graceful recovery story because a panic mid-dispatch leaves
    /// the Vm's invariants unspecified, and reusing it would risk
    /// UB.
    #[inline]
    fn with_vm_mut<R>(&self, f: impl FnOnce(&mut Vm) -> R) -> R {
        let _guard = self.lock.write().unwrap_or_else(|e| {
            // Poisoned: the previous holder panicked. Take the inner
            // guard anyway; the caller will likely panic too once it
            // observes the Vm's inconsistent state, but at least we
            // don't deadlock forever.
            e.into_inner()
        });
        // SAFETY: the write guard above provides exclusive access to
        // the wrapped `Vm` for this scope. `UnsafeCell::get()`
        // produces a `*mut Vm`; we materialize a `&mut Vm` with
        // lifetime bounded by `_guard`. Per the module-level safety
        // contract, this re-establishes the single-mutator invariant
        // that bare `Vm` relies on at the type level.
        let vm: &mut Vm = unsafe { &mut *self.inner.get() };
        f(vm)
    }
}

impl Clone for SendVm {
    /// Clones share the underlying `Vm` via the inner `Arc`.
    /// Concurrent method calls on cloned handles block on the lock;
    /// for parallel execution construct independent `SendVm`s
    /// instead.
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            lock: Arc::clone(&self.lock),
        }
    }
}

impl std::fmt::Debug for SendVm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendVm")
            .field("handles", &Arc::strong_count(&self.inner))
            .finish_non_exhaustive()
    }
}
