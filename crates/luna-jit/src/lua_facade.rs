//! mlua-style `Lua` facade.
//!
//! A thin wrapper around [`luna_core::vm::Vm`] that exposes the same
//! API in a shape familiar to embedders coming from `rlua` / `mlua`:
//!
//! ```
//! use luna_jit::Lua;
//!
//! let mut lua = Lua::new();
//! lua.open_base();
//! lua.open_math();
//! let r: i64 = lua.eval("return 1 + 2").unwrap();
//! assert_eq!(r, 3);
//!
//! let add = lua.create_function(|a: i64, b: i64| -> i64 { a + b });
//! lua.set_global("add", add).unwrap();
//! let r: i64 = lua.eval("return add(40, 2)").unwrap();
//! assert_eq!(r, 42);
//! ```
//!
//! ## Handles
//!
//! [`LuaFunction`] / [`LuaTable`] / [`LuaRoot`] are `Copy` wrappers
//! around a [`HostRootTicket`] returned by [`Vm::pin_host`]. They
//! keep their referenced `Gc<T>` alive across calls (so a `LuaTable`
//! survives a GC cycle even when no Lua-side reference exists).
//!
//! Slots are recycled — a single handle can be
//! released via [`Lua::unpin`]; the whole batch via
//! [`Lua::unpin_all`]. Both operations bump the slot's generation,
//! invalidating any further use of `LuaFunction` / `LuaTable` /
//! `LuaRoot` `Copy` values that referenced the released slot
//! (subsequent reads / calls panic on the stale ticket).
//!
//! ## Threading
//!
//! `Lua` inherits `Vm`'s `!Send + !Sync` contract. See
//! [`docs/threading.md`](../../../../docs/threading.md) for canonical
//! embedding patterns.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{
    FromLuaValue, HostRootStale, HostRootTicket, IntoValue, LuaError, NativeTypedSig,
    SandboxBuilder, Vm,
};

/// `mlua`-style front door for embedders. Wraps a [`Vm`] with JIT
/// installed by default (`Vm::new_minimal_with_jit`).
pub struct Lua(Vm);

impl Lua {
    /// Create a Lua VM with JIT installed + Lua 5.5 dialect.
    pub fn new() -> Lua {
        Lua(crate::new_minimal_with_jit(LuaVersion::Lua55))
    }

    /// Pick a specific dialect (5.1-5.5).
    pub fn with_version(v: LuaVersion) -> Lua {
        Lua(crate::new_minimal_with_jit(v))
    }

    /// A VM of dialect `v` that shares compiled code with the other VMs
    /// of `engine` (see [`crate::Engine`]).
    pub fn with_engine(engine: &crate::Engine, v: LuaVersion) -> Lua {
        Lua(engine.new_minimal_vm(v))
    }

    /// Sandbox-mode builder — same as [`Vm::sandbox`] but doesn't
    /// install JIT by default. `.build_lua()` finalizes to a `Lua`
    /// wrapping the sandboxed `Vm`.
    pub fn sandbox(v: LuaVersion) -> LuaSandboxBuilder {
        LuaSandboxBuilder {
            inner: Vm::sandbox(v),
        }
    }

    /// Borrow the underlying `Vm` for direct access (escape hatch
    /// for cases the facade doesn't cover).
    pub fn vm(&mut self) -> &mut Vm {
        &mut self.0
    }

    /// Open the base library (`print`, `type`, `pcall`, etc.).
    pub fn open_base(&mut self) {
        self.0.open_base();
    }

    /// Open the math library.
    pub fn open_math(&mut self) {
        self.0.open_math();
    }

    /// Open the string library.
    pub fn open_string(&mut self) {
        self.0.open_string();
    }

    /// Open the table library.
    pub fn open_table(&mut self) {
        self.0.open_table();
    }

    /// Open the coroutine library.
    pub fn open_coroutine(&mut self) {
        self.0.open_coroutine();
    }

    /// Compile and run `src`; extract the first return value as `T`.
    /// Use [`Lua::eval_multi`] to retrieve all returns.
    pub fn eval<T: FromLuaValue>(&mut self, src: &str) -> Result<T, LuaError> {
        let mut r = self.0.eval(src)?;
        if r.is_empty() {
            T::from_lua_value(Value::Nil)
        } else {
            T::from_lua_value(r.remove(0))
        }
    }

    /// Compile and run `src`; return all results.
    pub fn eval_multi(&mut self, src: &str) -> Result<Vec<Value>, LuaError> {
        self.0.eval(src)
    }

    /// Async variant of [`Lua::eval`]. Returns an `!Send` future that
    /// drives the dispatcher with cooperative yields on instruction
    /// budget exhaustion. Pin this to a `current_thread` Tokio
    /// runtime (or a `LocalSet` inside multi-thread Tokio) — see
    /// `docs/threading.md` and `examples/async_host.rs`.
    pub async fn eval_async<T: FromLuaValue>(&mut self, src: &str) -> Result<T, LuaError> {
        let mut r = self.0.eval_async(src).await?;
        if r.is_empty() {
            T::from_lua_value(Value::Nil)
        } else {
            T::from_lua_value(r.remove(0))
        }
    }

    /// Async variant of [`Lua::eval_multi`].
    pub async fn eval_async_multi(&mut self, src: &str) -> Result<Vec<Value>, LuaError> {
        self.0.eval_async(src).await
    }

    /// Register an async native function callable from Lua. The
    /// raw fn pointer ABI takes `(*mut Vm, func_slot, nargs)` and
    /// returns a boxed future — see [`luna_core::vm::AsyncNativeFn`]
    /// for the safety contract.
    ///
    /// Calling an async native from inside `vm.eval()` (sync mode)
    /// errors with a typed `LuaError`; embedders must drive the call
    /// through `eval_async`.
    pub fn set_async_native(
        &mut self,
        name: &str,
        f: luna_core::vm::AsyncNativeFn,
    ) -> Result<(), LuaError> {
        self.0.set_async_native(name, f)
    }

    /// Set a global by name. Accepts any [`IntoValue`] including
    /// `LuaFunction` / `LuaTable` / `LuaRoot` (the handle types impl
    /// `IntoValue` so they fan in alongside primitives + `Value`).
    pub fn set_global<V: IntoValue>(&mut self, name: &str, v: V) -> Result<(), LuaError> {
        self.0.set_global(name, v)
    }

    /// Borrow the globals table as a [`LuaTable`] handle.
    pub fn globals(&mut self) -> LuaTable {
        let g = self.0.globals();
        let ticket = self.0.pin_host(Value::Table(g));
        LuaTable { ticket }
    }

    /// Allocate a fresh empty table; return a handle that keeps it alive.
    pub fn create_table(&mut self) -> LuaTable {
        let t = self.0.new_table().build();
        let ticket = self.0.pin_host(Value::Table(t));
        LuaTable { ticket }
    }

    /// Wrap a typed Rust function as a Lua callable. See
    /// [`Vm::native_typed`] for the supported callable shapes.
    pub fn create_function<F, Marker>(&mut self, f: F) -> LuaFunction
    where
        F: NativeTypedSig<Marker>,
    {
        let v = self.0.native_typed(f);
        let ticket = self.0.pin_host(v);
        LuaFunction { ticket }
    }

    /// Pin an arbitrary value as a host root; the returned [`LuaRoot`]
    /// keeps it alive until [`Lua::unpin`] or [`Lua::unpin_all`].
    pub fn pin<V: IntoValue>(&mut self, v: V) -> LuaRoot {
        let v = v.into_value(&mut self.0);
        let ticket = self.0.pin_host(v);
        LuaRoot { ticket }
    }

    /// Release a single pinned handle. The handle's
    /// slot is recycled; the supplied `LuaFunction` / `LuaTable` /
    /// `LuaRoot` value (and any `Copy`-cloned aliases) becomes stale
    /// and will panic on subsequent reads / calls.
    ///
    /// Returns `Err(HostRootStale)` if the handle was already
    /// released — pool is unchanged in that case, so embedders can
    /// safely ignore the error if double-unpin is acceptable.
    pub fn unpin<H: PinnedHandle>(&mut self, h: H) -> Result<(), HostRootStale> {
        self.0.unpin(h.ticket())
    }

    /// Drop every pinned handle. `LuaFunction` / `LuaTable` /
    /// `LuaRoot` created before this call become invalid (panic on
    /// use). Bumps every slot's generation; underlying `Vec` capacity
    /// is retained for amortized future allocations.
    pub fn unpin_all(&mut self) {
        self.0.unpin_all();
    }

    /// Number of currently-pinned handles (diagnostic). Counts live (non-free) slots, so a steady `pin → unpin`
    /// loop holds at 1 instead of growing monotonically.
    pub fn pinned_count(&self) -> usize {
        self.0.host_root_count()
    }
}

/// Common trait for handle types that wrap a
/// [`HostRootTicket`]. Lets [`Lua::unpin`] accept `LuaFunction` /
/// `LuaTable` / `LuaRoot` uniformly.
pub trait PinnedHandle {
    /// The ticket this handle wraps.
    fn ticket(&self) -> HostRootTicket;
}

impl Default for Lua {
    fn default() -> Self {
        Lua::new()
    }
}

/// Sandbox builder that finalizes to a `Lua` (instead of a bare `Vm`).
pub struct LuaSandboxBuilder {
    inner: SandboxBuilder,
}

impl LuaSandboxBuilder {
    /// Whitelist the `base` standard library.
    pub fn open_base(mut self) -> Self {
        self.inner = self.inner.open_base();
        self
    }
    /// Whitelist the `math` standard library.
    pub fn open_math(mut self) -> Self {
        self.inner = self.inner.open_math();
        self
    }
    /// Whitelist the `string` standard library.
    pub fn open_string(mut self) -> Self {
        self.inner = self.inner.open_string();
        self
    }
    /// Whitelist the `table` standard library.
    pub fn open_table(mut self) -> Self {
        self.inner = self.inner.open_table();
        self
    }
    /// Whitelist the `coroutine` standard library.
    pub fn open_coroutine(mut self) -> Self {
        self.inner = self.inner.open_coroutine();
        self
    }
    /// Cap the instruction count; once exhausted, the Vm raises the error
    /// on every instruction until `Vm::set_instr_budget` arms a new one.
    pub fn with_instr_budget(mut self, n: i64) -> Self {
        self.inner = self.inner.with_instr_budget(n);
        self
    }
    /// Cap heap memory (approximate; see [`crate::vm::Vm::set_memory_cap`]).
    pub fn with_memory_cap(mut self, n: usize) -> Self {
        self.inner = self.inner.with_memory_cap(n);
        self
    }
    /// Re-enable precompiled-bytecode loading (off by default in sandbox
    /// mode for safety).
    pub fn allow_bytecode_loading(mut self) -> Self {
        self.inner = self.inner.allow_bytecode_loading();
        self
    }
    /// Finalize the builder and return a configured [`Lua`].
    pub fn build(self) -> Lua {
        Lua(self.inner.build())
    }
}

mod handles;
pub use handles::{IntoLuaArgs, LuaFunction, LuaRoot, LuaTable};
