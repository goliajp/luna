//! mlua-style `Lua` facade (B12, Phase 2 P2-D).
//!
//! A thin wrapper around [`luna_core::Vm`] that exposes the same
//! API in a shape familiar to embedders coming from `rlua` / `mlua`:
//!
//! ```
//! use luna::Lua;
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
//! [`LuaFunction`] / [`LuaTable`] / [`LuaRoot<V>`] are `Copy` wrappers
//! around an index into [`Vm::pin_host`]'s pool. They keep their
//! referenced `Gc<T>` alive across calls (so a `LuaTable` survives
//! a GC cycle even when no Lua-side reference exists). v1.1 ships
//! append-only pin semantics; release the entire pool with
//! [`Lua::unpin_all`]. Slot recycling lands in Phase 3 alongside B8.
//!
//! ## Threading
//!
//! `Lua` inherits `Vm`'s `!Send + !Sync` contract. See
//! [`docs/threading.md`](../../../../docs/threading.md) for canonical
//! embedding patterns.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{FromLuaValue, IntoValue, LuaError, NativeTypedSig, SandboxBuilder, Vm};

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

    /// Set a global by name. Accepts any [`IntoValue`] including
    /// `LuaFunction` / `LuaTable` / `LuaRoot` (the handle types impl
    /// `IntoValue` so they fan in alongside primitives + `Value`).
    pub fn set_global<V: IntoValue>(&mut self, name: &str, v: V) -> Result<(), LuaError> {
        self.0.set_global(name, v)
    }

    /// Borrow the globals table as a [`LuaTable`] handle.
    pub fn globals(&mut self) -> LuaTable {
        let g = self.0.globals();
        let idx = self.0.pin_host(Value::Table(g));
        LuaTable { idx }
    }

    /// Allocate a fresh empty table; return a handle that keeps it alive.
    pub fn create_table(&mut self) -> LuaTable {
        let t = self.0.new_table().build();
        let idx = self.0.pin_host(Value::Table(t));
        LuaTable { idx }
    }

    /// Wrap a typed Rust function as a Lua callable. See
    /// [`Vm::native_typed`] for the supported callable shapes.
    pub fn create_function<F, Marker>(&mut self, f: F) -> LuaFunction
    where
        F: NativeTypedSig<Marker>,
    {
        let v = self.0.native_typed(f);
        let idx = self.0.pin_host(v);
        LuaFunction { idx }
    }

    /// Pin an arbitrary value as a host root; the returned [`LuaRoot`]
    /// keeps it alive until [`Lua::unpin_all`].
    pub fn pin<V: IntoValue>(&mut self, v: V) -> LuaRoot {
        let v = v.into_value(&mut self.0);
        let idx = self.0.pin_host(v);
        LuaRoot { idx }
    }

    /// Drop every pinned handle. `LuaFunction` / `LuaTable` / `LuaRoot`
    /// created before this call become invalid (panic on use).
    pub fn unpin_all(&mut self) {
        self.0.unpin_all();
    }

    /// Number of currently-pinned handles (diagnostic).
    pub fn pinned_count(&self) -> usize {
        self.0.host_root_count()
    }
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
    pub fn open_base(mut self) -> Self {
        self.inner = self.inner.open_base();
        self
    }
    pub fn open_math(mut self) -> Self {
        self.inner = self.inner.open_math();
        self
    }
    pub fn open_string(mut self) -> Self {
        self.inner = self.inner.open_string();
        self
    }
    pub fn open_table(mut self) -> Self {
        self.inner = self.inner.open_table();
        self
    }
    pub fn open_coroutine(mut self) -> Self {
        self.inner = self.inner.open_coroutine();
        self
    }
    pub fn with_instr_budget(mut self, n: i64) -> Self {
        self.inner = self.inner.with_instr_budget(n);
        self
    }
    pub fn with_memory_cap(mut self, n: usize) -> Self {
        self.inner = self.inner.with_memory_cap(n);
        self
    }
    pub fn allow_bytecode_loading(mut self) -> Self {
        self.inner = self.inner.allow_bytecode_loading();
        self
    }
    pub fn build(self) -> Lua {
        Lua(self.inner.build())
    }
}

// ─────────────────────────────────────────────────────────────────────
// Handle types
// ─────────────────────────────────────────────────────────────────────

/// Handle to a Lua-callable value (`Value::Closure` or
/// `Value::Native`) pinned in the host root pool. `Copy`-able —
/// clones share the same pinned slot.
#[derive(Copy, Clone, Debug)]
pub struct LuaFunction {
    idx: usize,
}

impl LuaFunction {
    /// Call this function with the given typed args; decode the
    /// (first) return as `R`. Use [`LuaFunction::call_multi`] for
    /// the full result vector.
    pub fn call<A, R>(self, lua: &mut Lua, args: A) -> Result<R, LuaError>
    where
        A: IntoLuaArgs,
        R: FromLuaValue,
    {
        let f = lua.0.host_root_at(self.idx);
        let args = args.into_lua_args(&mut lua.0);
        let mut r = lua.0.call_value(f, &args)?;
        if r.is_empty() {
            R::from_lua_value(Value::Nil)
        } else {
            R::from_lua_value(r.remove(0))
        }
    }

    /// Call this function; return all results.
    pub fn call_multi<A>(self, lua: &mut Lua, args: A) -> Result<Vec<Value>, LuaError>
    where
        A: IntoLuaArgs,
    {
        let f = lua.0.host_root_at(self.idx);
        let args = args.into_lua_args(&mut lua.0);
        lua.0.call_value(f, &args)
    }
}

impl IntoValue for LuaFunction {
    fn into_value(self, vm: &mut Vm) -> Value {
        vm.host_root_at(self.idx)
    }
}

/// Handle to a `Value::Table` pinned in the host root pool.
#[derive(Copy, Clone, Debug)]
pub struct LuaTable {
    idx: usize,
}

impl LuaTable {
    /// Set `t[k] = v`. Both `k` and `v` may be any [`IntoValue`].
    pub fn set<K: IntoValue, V: IntoValue>(
        self,
        lua: &mut Lua,
        k: K,
        v: V,
    ) -> Result<(), LuaError> {
        let t = match lua.0.host_root_at(self.idx) {
            Value::Table(t) => t,
            _ => return Err(LuaError(Value::Nil)),
        };
        let k = k.into_value(&mut lua.0);
        let v = v.into_value(&mut lua.0);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is
        // single-threaded (see heap.rs:5-7).
        unsafe { t.as_mut() }.set(&mut lua.0.heap, k, v)?;
        lua.0
            .heap
            .barrier_back(t.as_ptr() as *mut luna_core::runtime::heap::GcHeader);
        Ok(())
    }

    /// Read `t[k]`; decode as `V`. Returns `Err` if the key is
    /// missing OR the value's type doesn't match `V`. Use
    /// `t.raw_get(k)` (returning `Value`) for runtime branching.
    pub fn get<K: IntoValue, V: FromLuaValue>(
        self,
        lua: &mut Lua,
        k: K,
    ) -> Result<V, LuaError> {
        let v = self.raw_get(lua, k)?;
        V::from_lua_value(v)
    }

    /// Read `t[k]` as a raw [`Value`] (no type coercion).
    pub fn raw_get<K: IntoValue>(self, lua: &mut Lua, k: K) -> Result<Value, LuaError> {
        let t = match lua.0.host_root_at(self.idx) {
            Value::Table(t) => t,
            _ => return Err(LuaError(Value::Nil)),
        };
        let k = k.into_value(&mut lua.0);
        // SAFETY: see set() — same single-threaded GC contract.
        Ok(unsafe { t.as_mut() }.get(k))
    }
}

impl IntoValue for LuaTable {
    fn into_value(self, vm: &mut Vm) -> Value {
        vm.host_root_at(self.idx)
    }
}

/// Generic pinned root. Use for arbitrary `Value`s the embedder
/// wants to keep alive without wrapping in `LuaFunction` / `LuaTable`.
#[derive(Copy, Clone, Debug)]
pub struct LuaRoot {
    idx: usize,
}

impl LuaRoot {
    /// Read the pinned value.
    pub fn get(self, lua: &Lua) -> Value {
        lua.0.host_root_at(self.idx)
    }
}

impl IntoValue for LuaRoot {
    fn into_value(self, vm: &mut Vm) -> Value {
        vm.host_root_at(self.idx)
    }
}

// ─────────────────────────────────────────────────────────────────────
// IntoLuaArgs — tuple to &[Value] conversion for LuaFunction::call
// ─────────────────────────────────────────────────────────────────────

/// Convert a tuple of typed values into the `&[Value]` shape
/// [`Vm::call_value`] expects. Implemented for `()` + tuples of
/// [`IntoValue`] up to arity 6.
pub trait IntoLuaArgs {
    fn into_lua_args(self, vm: &mut Vm) -> Vec<Value>;
}

impl IntoLuaArgs for () {
    fn into_lua_args(self, _vm: &mut Vm) -> Vec<Value> {
        Vec::new()
    }
}

macro_rules! impl_into_lua_args_tuple {
    ( $( ($($name:ident: $idx:tt),+) ),+ $(,)? ) => {
        $(
            impl<$($name: IntoValue),+> IntoLuaArgs for ($($name,)+) {
                fn into_lua_args(self, vm: &mut Vm) -> Vec<Value> {
                    vec![ $( self.$idx.into_value(vm), )+ ]
                }
            }
        )+
    };
}
impl_into_lua_args_tuple! {
    (T0: 0),
    (T0: 0, T1: 1),
    (T0: 0, T1: 1, T2: 2),
    (T0: 0, T1: 1, T2: 2, T3: 3),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5),
}
