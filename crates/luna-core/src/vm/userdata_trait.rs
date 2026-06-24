//! `LuaUserdata` trait sugar (v1.2 Track B).
//!
//! Layered on top of v1.1 B8 (`UserdataPayload::Host`, `Vm::create_userdata`,
//! `Vm::userdata_borrow`). The B8 base lets embedders stash a `T: 'static`
//! Rust value inside a `Value::Userdata`; this module is what makes that
//! userdata *callable from Lua* — methods, metamethods, and a cached
//! per-Vm metatable.
//!
//! ```
//! use luna_core::vm::{LuaUserdata, MetaMethod, UserdataMethods, Vm};
//! use luna_core::version::LuaVersion;
//!
//! struct Counter { value: i64 }
//!
//! impl LuaUserdata for Counter {
//!     fn type_name() -> &'static str { "Counter" }
//!     fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
//!         m.add_method("get", |_vm, this, ()| Ok::<_, _>(this.value));
//!         m.add_method_mut("incr", |_vm, this, (by,): (i64,)| {
//!             this.value += by;
//!             Ok::<_, _>(())
//!         });
//!         m.add_meta_method(MetaMethod::ToString, |_vm, this, ()| {
//!             Ok::<_, _>(format!("Counter({})", this.value))
//!         });
//!     }
//! }
//!
//! let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
//! vm.set_userdata("c", Counter { value: 100 }).unwrap();
//! vm.eval("c:incr(50)").unwrap();
//! let r = vm.eval("return c:get()").unwrap();
//! assert!(matches!(r[0], luna_core::runtime::Value::Int(150)));
//! ```
//!
//! The trait + builder live in `luna-core` (alongside `typed_native.rs`)
//! because nothing here depends on JIT-bearing types: dispatch routes
//! through the existing metatable plumbing (`exec.rs::metatable_of` /
//! `get_mm` / `check_finalizer_userdata`), and trampolines reuse the
//! `pack` / `reconstruct` machinery from `typed_native.rs`.

use std::any::TypeId;
use std::marker::PhantomData;

use crate::runtime::heap::{Gc, GcHeader};
use crate::runtime::table::Table;
use crate::runtime::value::{NativeFn, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::typed_native::{FromLuaArgs, IntoLuaReturn};

// ─────────────────────────────────────────────────────────────────────
// UserdataMarker — public facade over the GC marker passed to
// `LuaUserdata::trace`. Phase TB (v1.3).
// ─────────────────────────────────────────────────────────────────────

/// Public facade over the GC mark accumulator passed to
/// [`LuaUserdata::trace`].
///
/// Wraps the crate-internal [`crate::runtime::heap::Marker`] so embedders
/// never see the gray-stack / weak-table internals. Holds a mutable
/// borrow of the underlying marker for the duration of a single trace
/// call. Constructed only by the collector via the crate-internal
/// `__new_internal` constructor; embedders cannot synthesize one outside
/// a trace call.
///
/// ## Trace-method contract
///
/// Inside [`LuaUserdata::trace`] the embedder may **only**:
/// - call [`UserdataMarker::mark`] / [`UserdataMarker::mark_value`] on
///   `Gc<...>` handles / `Value`s reachable from `&self`
/// - read fields of `&self`
///
/// The embedder must **not** allocate new GC objects, reenter the `Vm`,
/// take locks, or perform I/O. The trace call runs synchronously inside
/// the collector's mark phase and must return in bounded wall time.
pub struct UserdataMarker<'a> {
    inner: &'a mut crate::runtime::heap::Marker,
}

impl<'a> UserdataMarker<'a> {
    /// Crate-internal constructor. Not part of the public API — only
    /// the collector (`Userdata::trace`) builds one.
    #[doc(hidden)]
    pub(crate) fn __new_internal(inner: &'a mut crate::runtime::heap::Marker) -> Self {
        UserdataMarker { inner }
    }

    /// Mark a Gc-managed object as reachable. Returns `true` on the
    /// first visit (white → gray transition). Idempotent on later
    /// visits within the same cycle.
    pub fn mark<T>(&mut self, g: Gc<T>) -> bool {
        self.inner.header(g.as_ptr() as *mut GcHeader)
    }

    /// Convenience: mark every Gc-managed object referenced by a
    /// [`Value`]. No-op for primitive variants (`Int`, `Float`,
    /// `Bool`, `Nil`, `LightUserdata`).
    pub fn mark_value(&mut self, v: Value) -> bool {
        self.inner.value(v)
    }
}

// ─────────────────────────────────────────────────────────────────────
// MetaMethod — public-facing metamethod tag
// ─────────────────────────────────────────────────────────────────────

/// Public metamethod kinds for [`UserdataMethods::add_meta_method`].
///
/// Maps 1:1 onto the dispatcher's internal `Mm` enum. Listed
/// explicitly so the public surface doesn't leak `Mm`'s discriminant
/// layout — `Mm` stays `pub(crate)` in `exec.rs`.
///
/// Not all `Mm` variants are exposed: `Mm::Metatable` (the `__metatable`
/// guard) and `Mm::Name` are set indirectly via [`LuaUserdata::type_name`]
/// and `getmetatable`; surfacing them as `add_meta_method` targets
/// would be confusing.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MetaMethod {
    /// `__add` — binary `+`.
    Add,
    /// `__sub` — binary `-`.
    Sub,
    /// `__mul` — binary `*`.
    Mul,
    /// `__div` — binary `/`.
    Div,
    /// `__mod` — binary `%`.
    Mod,
    /// `__pow` — binary `^`.
    Pow,
    /// `__idiv` — binary `//`.
    IDiv,
    /// `__band` — binary `&`.
    BAnd,
    /// `__bor` — binary `|`.
    BOr,
    /// `__bxor` — binary `~` (bitwise xor).
    BXor,
    /// `__shl` — `<<`.
    Shl,
    /// `__shr` — `>>`.
    Shr,
    /// `__bnot` — unary `~`.
    BNot,
    /// `__unm` — unary `-`.
    Unm,
    /// `__concat` — binary `..`.
    Concat,
    /// `__len` — unary `#`.
    Len,
    /// `__eq` — `==`.
    Eq,
    /// `__lt` — `<`.
    Lt,
    /// `__le` — `<=`.
    Le,
    /// `__index` — non-existent key lookup. Setting this directly
    /// overrides the per-method dispatch table installed by
    /// [`UserdataMethods::add_method`] etc., so only use it when you
    /// want full control of the lookup; the trait's default `__index`
    /// is a table of `add_method` entries.
    Index,
    /// `__newindex` — non-existent key assignment.
    NewIndex,
    /// `__call` — `obj(args)`.
    Call,
    /// `__tostring` — `tostring(obj)`.
    ToString,
    /// `__pairs` — `pairs(obj)` (5.2+).
    Pairs,
    /// `__close` — to-be-closed handler (5.4+).
    Close,
    /// `__gc` — finalizer. **The metatable's `__gc` fires before
    /// Rust's `Drop` on the host payload.**
    Gc,
}

impl MetaMethod {
    /// Lua-side string spelling of this metamethod (`"__add"`, `"__gc"`, …).
    pub const fn name(self) -> &'static str {
        match self {
            MetaMethod::Add => "__add",
            MetaMethod::Sub => "__sub",
            MetaMethod::Mul => "__mul",
            MetaMethod::Div => "__div",
            MetaMethod::Mod => "__mod",
            MetaMethod::Pow => "__pow",
            MetaMethod::IDiv => "__idiv",
            MetaMethod::BAnd => "__band",
            MetaMethod::BOr => "__bor",
            MetaMethod::BXor => "__bxor",
            MetaMethod::Shl => "__shl",
            MetaMethod::Shr => "__shr",
            MetaMethod::BNot => "__bnot",
            MetaMethod::Unm => "__unm",
            MetaMethod::Concat => "__concat",
            MetaMethod::Len => "__len",
            MetaMethod::Eq => "__eq",
            MetaMethod::Lt => "__lt",
            MetaMethod::Le => "__le",
            MetaMethod::Index => "__index",
            MetaMethod::NewIndex => "__newindex",
            MetaMethod::Call => "__call",
            MetaMethod::ToString => "__tostring",
            MetaMethod::Pairs => "__pairs",
            MetaMethod::Close => "__close",
            MetaMethod::Gc => "__gc",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// LuaUserdata + UserdataMethods traits
// ─────────────────────────────────────────────────────────────────────

/// Embedder-side trait: implement on any `T: 'static` to expose
/// method-rich Lua userdata via `vm.set_userdata::<T>(...)`.
///
/// The trait's only required method is [`add_methods`], which defaults
/// to registering nothing — yielding a userdata that still type-checks
/// as `"userdata"` but only carries identity + `__name`. An empty impl
/// (`impl LuaUserdata for MyType {}`) is the source-compatible bridge
/// for B8 callers upgrading from v1.1.
///
/// [`add_methods`]: LuaUserdata::add_methods
///
/// ## v1.1 → v1.2 migration
///
/// v1.1 [`Vm::create_userdata`] / [`Vm::set_userdata`] accepted any
/// `T: Any + 'static`; v1.2 narrows the bound to `T: LuaUserdata`. Any
/// existing type carries over with a one-line empty impl:
///
/// ```
/// # use luna_core::vm::LuaUserdata;
/// struct MyType { /* … */ }
/// impl LuaUserdata for MyType {}
/// ```
///
/// ## Contract on the host payload
///
/// `T` may hold `Gc<...>` fields **provided it overrides [`trace`]** to
/// mark every such handle. The default [`trace`] is a no-op, suitable
/// for pure host types (no Gc-managed inner state). Forgetting to
/// override [`trace`] when `T` carries a `Gc<Table>` / `Gc<LuaStr>` /
/// `Gc<NativeClosure>` / `Gc<Coro>` / `Gc<Userdata>` field whose
/// lifetime is not otherwise rooted risks dangling references after
/// collection.
///
/// v1.2 forbade Gc-bearing payloads entirely; v1.3 Phase TB lifts the
/// limitation by giving the trait a default [`trace`] method and
/// storing a monomorphic adapter in [`crate::runtime::userdata::UserdataPayload::Host`].
///
/// [`trace`]: LuaUserdata::trace
pub trait LuaUserdata: 'static + Sized {
    /// Lua-visible type name. Used as the `__name` field of the
    /// generated metatable; surfaces in tostring fallback messages and
    /// in PUC-style `"attempt to index a Counter value"` errors.
    /// Defaults to [`std::any::type_name`].
    fn type_name() -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Register methods + metamethods on `m`. Called exactly once per
    /// `T` per `Vm`, at the first
    /// [`Vm::create_userdata::<T>`](Vm::create_userdata) /
    /// [`set_userdata::<T>`](Vm::set_userdata) — the resulting
    /// metatable is cached on the Vm keyed by `TypeId::of::<T>()`.
    fn add_methods<M: UserdataMethods<Self>>(_m: &mut M) {}

    /// Mark every Gc-managed handle reachable from `self`. The default
    /// is a no-op — override only when `T` directly holds
    /// `Gc<Table>` / `Gc<LuaStr>` / `Gc<NativeClosure>` / `Gc<Coro>` /
    /// `Gc<Userdata>` fields whose lifetime is not otherwise rooted
    /// (i.e. not pinned via [`Vm::pin_host`] and not reachable from a
    /// Lua-side table).
    ///
    /// Called by the collector during the mark phase; the call runs
    /// synchronously, single-threaded, and must return in bounded wall
    /// time. The embedder must not allocate new GC objects, reenter the
    /// `Vm`, take locks, or perform I/O from inside `trace` — see the
    /// [`UserdataMarker`] type docs for the full contract.
    ///
    /// ## Override example
    ///
    /// ```ignore
    /// use luna_core::runtime::{Gc, Table};
    /// use luna_core::vm::{LuaUserdata, UserdataMarker};
    ///
    /// struct Cache { entries: Gc<Table> }
    /// impl LuaUserdata for Cache {
    ///     fn trace(&self, m: &mut UserdataMarker) {
    ///         m.mark(self.entries);
    ///     }
    /// }
    /// ```
    ///
    /// Overriding `trace` does not require touching any other trait
    /// method; existing B8 / v1.2 types remain source-compatible with
    /// an unchanged empty `impl LuaUserdata for T {}` (the default
    /// no-op runs and no Gc tracing is performed).
    fn trace(&self, _m: &mut UserdataMarker) {}
}

/// Builder passed to [`LuaUserdata::add_methods`]. The concrete impl
/// is [`MetatableBuilder<T>`] (in this module) — `UserdataMethods` is
/// a trait only to keep the `M:` bound usable from generic code.
pub trait UserdataMethods<T> {
    /// Register a regular method bound to `__index[name]` on the
    /// generated metatable; method lookup `u:name(args)` resolves
    /// through Lua's normal `__index` table dispatch.
    fn add_method<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static;

    /// Mutable variant of [`add_method`](Self::add_method). The
    /// `&mut T` borrow is exclusive within the call window; an
    /// embedder must not concurrently `userdata_borrow_mut` the same
    /// payload through another path during the method body.
    fn add_method_mut<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static;

    /// Register a static-style function (no implicit receiver). Bound
    /// directly on the metatable, not under `__index`, so it is
    /// reachable as `Vec3.new(...)` after `vm.set_global("Vec3", mt)`.
    fn add_function<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static;

    /// Register a metamethod (`__add` / `__tostring` / …). Stored
    /// directly on the metatable; the dispatcher's existing
    /// `get_mm` path resolves it.
    fn add_meta_method<F, A, R>(&mut self, meta: MetaMethod, f: F)
    where
        F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static;

    /// Mutable variant of [`add_meta_method`](Self::add_meta_method).
    fn add_meta_method_mut<F, A, R>(&mut self, meta: MetaMethod, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static;

    /// Field-getter sugar: equivalent to [`add_method`](Self::add_method)
    /// with no args and a single-value return.
    ///
    /// **v1.2 limitation**: the resulting accessor uses call-syntax
    /// (`obj:name()`). True field-style `obj.name` (no parentheses)
    /// requires `__index` as a function dispatcher and is a v1.3
    /// polish item; documented in `docs/embedding.md` §7.
    fn add_field_method_get<F, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &T) -> Result<R, LuaError> + Copy + 'static,
        R: IntoLuaReturn + 'static;
}

// ─────────────────────────────────────────────────────────────────────
// MetatableBuilder<T> — the concrete UserdataMethods<T> impl
// ─────────────────────────────────────────────────────────────────────

/// Concrete builder that emits a [`Gc<Table>`] metatable for `T`.
/// Created internally by [`Vm::register_userdata`]; embedders never
/// name this type.
pub struct MetatableBuilder<'vm, T> {
    vm: &'vm mut Vm,
    /// `__index` sub-table entries (regular methods).
    methods: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    /// Direct metatable entries (metamethods + static functions).
    meta_entries: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    _phantom: PhantomData<fn() -> T>,
}

impl<'vm, T: LuaUserdata> MetatableBuilder<'vm, T> {
    fn new(vm: &'vm mut Vm) -> Self {
        Self {
            vm,
            methods: Vec::new(),
            meta_entries: Vec::new(),
            _phantom: PhantomData,
        }
    }

    fn intern(&mut self, s: &str) -> Gc<crate::runtime::LuaStr> {
        self.vm.heap.intern(s.as_bytes())
    }

    fn make_native(&mut self, f: NativeFn, upvals: Box<[Value]>) -> Value {
        self.vm.native_with(f, upvals)
    }

    /// Build the metatable from the accumulated entries. Called by
    /// [`Vm::register_userdata`] after [`LuaUserdata::add_methods`] returns.
    fn finalize(self) -> Result<Gc<Table>, LuaError> {
        let MetatableBuilder {
            vm,
            methods,
            meta_entries,
            ..
        } = self;

        let mt = vm.heap.new_table();
        // __name — drives PUC-style error messages.
        let name_key = vm.heap.intern(b"__name");
        let name_val = Value::Str(vm.heap.intern(T::type_name().as_bytes()));
        // SAFETY: mt is a fresh Gc<Table>; the heap is single-threaded.
        unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(name_key), name_val)?;

        // __index — a sub-table of regular methods. Only allocate if
        // any methods were registered; embedders who only set
        // metamethods (e.g. arithmetic on Vec3) skip the empty table.
        if !methods.is_empty() {
            let idx = vm.heap.new_table();
            for (k, v) in methods {
                // SAFETY: idx is freshly allocated.
                unsafe { idx.as_mut() }.set(&mut vm.heap, Value::Str(k), v)?;
            }
            let key = vm.heap.intern(b"__index");
            // SAFETY: see above.
            unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(key), Value::Table(idx))?;
            vm.heap
                .barrier_back(mt.as_ptr() as *mut crate::runtime::heap::GcHeader);
        }

        // Direct metatable entries (metamethods + static fns).
        for (k, v) in meta_entries {
            unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(k), v)?;
        }
        vm.heap
            .barrier_back(mt.as_ptr() as *mut crate::runtime::heap::GcHeader);

        Ok(mt)
    }
}

impl<'vm, T: LuaUserdata> UserdataMethods<T> for MetatableBuilder<'vm, T> {
    fn add_method<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_method::<T, F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.methods.push((k, v));
    }

    fn add_method_mut<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_method_mut::<T, F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.methods.push((k, v));
    }

    fn add_function<F, A, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_function::<F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.meta_entries.push((k, v));
    }

    fn add_meta_method<F, A, R>(&mut self, meta: MetaMethod, f: F)
    where
        F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_method::<T, F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(meta.name());
        self.meta_entries.push((k, v));
    }

    fn add_meta_method_mut<F, A, R>(&mut self, meta: MetaMethod, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<R, LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
        R: IntoLuaReturn + 'static,
    {
        let (raw_fn, upvals) = pack_method_mut::<T, F, A, R>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(meta.name());
        self.meta_entries.push((k, v));
    }

    fn add_field_method_get<F, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &T) -> Result<R, LuaError> + Copy + 'static,
        R: IntoLuaReturn + 'static,
    {
        // Adapt to add_method's (this, args) shape with A = ().
        let adapter = move |vm: &mut Vm, this: &T, _args: ()| f(vm, this);
        self.add_method(name, adapter);
    }
}

// ─────────────────────────────────────────────────────────────────────
// Trampolines + pack helpers
// ─────────────────────────────────────────────────────────────────────

/// Trampoline for `add_method` (`&T` self).
fn method_trampoline<T, F, A, R>(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError>
where
    T: LuaUserdata,
    F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
    A: FromLuaArgs + 'static,
    R: IntoLuaReturn + 'static,
{
    let f: F = reconstruct_zst_or_fnptr(vm, fs);
    let self_val = vm.nat_arg(fs, nargs, 0);
    let ud_gc = match self_val {
        Value::Userdata(g) => g,
        _ => {
            return Err(vm.rt_err(&format!(
                "method called on non-userdata value (expected {})",
                T::type_name()
            )));
        }
    };
    // Take a raw pointer up front so the borrow isn't tied to vm.
    let ud_ptr = ud_gc.as_ptr();
    // SAFETY: single-threaded GC heap; the Userdata at `ud_ptr` is
    // pinned by being on the Lua stack at slot `fs`.
    let type_matches = unsafe { (*ud_ptr).downcast::<T>().is_some() };
    if !type_matches {
        return Err(vm.rt_err(&format!(
            "method called on wrong userdata type (expected {})",
            T::type_name()
        )));
    }
    let args = A::from_lua_args_skip_self(vm, fs, nargs)?;
    // SAFETY: type_matches is true; the &T borrow is independent of `vm`.
    let this: &T = unsafe { (*ud_ptr).downcast::<T>().unwrap_unchecked() };
    f(vm, this, args).into_lua_return(vm, fs)
}

/// Trampoline for `add_method_mut` (`&mut T` self).
fn method_mut_trampoline<T, F, A, R>(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError>
where
    T: LuaUserdata,
    F: Fn(&mut Vm, &mut T, A) -> Result<R, LuaError> + Copy + 'static,
    A: FromLuaArgs + 'static,
    R: IntoLuaReturn + 'static,
{
    let f: F = reconstruct_zst_or_fnptr(vm, fs);
    let self_val = vm.nat_arg(fs, nargs, 0);
    let ud_gc = match self_val {
        Value::Userdata(g) => g,
        _ => {
            return Err(vm.rt_err(&format!(
                "method called on non-userdata value (expected {})",
                T::type_name()
            )));
        }
    };
    let ud_ptr = ud_gc.as_ptr();
    // SAFETY: see method_trampoline.
    let type_matches = unsafe { (*ud_ptr).downcast::<T>().is_some() };
    if !type_matches {
        return Err(vm.rt_err(&format!(
            "method called on wrong userdata type (expected {})",
            T::type_name()
        )));
    }
    let args = A::from_lua_args_skip_self(vm, fs, nargs)?;
    // SAFETY: see method_trampoline. The &mut T is exclusive within
    // this trampoline; embedders must not concurrently borrow the
    // same userdata payload through another API during the call.
    let this: &mut T = unsafe { (*ud_ptr).downcast_mut::<T>().unwrap_unchecked() };
    f(vm, this, args).into_lua_return(vm, fs)
}

/// Trampoline for `add_function` (no self).
fn function_trampoline<F, A, R>(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError>
where
    F: Fn(&mut Vm, A) -> Result<R, LuaError> + Copy + 'static,
    A: FromLuaArgs + 'static,
    R: IntoLuaReturn + 'static,
{
    let f: F = reconstruct_zst_or_fnptr(vm, fs);
    let args = A::from_lua_args(vm, fs, nargs)?;
    f(vm, args).into_lua_return(vm, fs)
}

fn pack_method<T, F, A, R>(f: F) -> (NativeFn, Box<[Value]>)
where
    T: LuaUserdata,
    F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
    A: FromLuaArgs + 'static,
    R: IntoLuaReturn + 'static,
{
    (method_trampoline::<T, F, A, R>, pack_zst_or_fnptr::<F>(f))
}

fn pack_method_mut<T, F, A, R>(f: F) -> (NativeFn, Box<[Value]>)
where
    T: LuaUserdata,
    F: Fn(&mut Vm, &mut T, A) -> Result<R, LuaError> + Copy + 'static,
    A: FromLuaArgs + 'static,
    R: IntoLuaReturn + 'static,
{
    (
        method_mut_trampoline::<T, F, A, R>,
        pack_zst_or_fnptr::<F>(f),
    )
}

fn pack_function<F, A, R>(f: F) -> (NativeFn, Box<[Value]>)
where
    F: Fn(&mut Vm, A) -> Result<R, LuaError> + Copy + 'static,
    A: FromLuaArgs + 'static,
    R: IntoLuaReturn + 'static,
{
    (function_trampoline::<F, A, R>, pack_zst_or_fnptr::<F>(f))
}

/// Mirror of [`crate::vm::typed_native`]'s private `pack` — kept
/// internal to this module to avoid widening that module's API.
#[inline]
fn pack_zst_or_fnptr<F: Copy + 'static>(f: F) -> Box<[Value]> {
    if std::mem::size_of::<F>() == 0 {
        Box::new([])
    } else {
        assert!(
            std::mem::size_of::<F>() == std::mem::size_of::<*const ()>(),
            "LuaUserdata method closure must be ZST (non-capturing) or fn-pointer-sized; \
             capturing closures unsupported in v1.2"
        );
        // SAFETY: F is fn-pointer-sized; transmute_copy stashes its
        // bytes as a raw *const () for storage. Recovered in
        // `reconstruct_zst_or_fnptr` below.
        let raw_ptr: *const () = unsafe { std::mem::transmute_copy(&f) };
        Box::new([Value::LightUserdata(raw_ptr)])
    }
}

#[inline]
fn reconstruct_zst_or_fnptr<F: Copy + 'static>(vm: &Vm, fs: u32) -> F {
    if std::mem::size_of::<F>() == 0 {
        // SAFETY: F is ZST.
        #[allow(clippy::uninit_assumed_init)]
        unsafe {
            std::mem::MaybeUninit::<F>::uninit().assume_init()
        }
    } else {
        let upval = vm.nat_upval(fs, 0);
        match upval {
            Value::LightUserdata(ptr) => {
                debug_assert_eq!(
                    std::mem::size_of::<F>(),
                    std::mem::size_of::<*const ()>(),
                    "non-ZST F must be fn-pointer-sized"
                );
                // SAFETY: stored via `pack_zst_or_fnptr` with the same F.
                unsafe { std::mem::transmute_copy::<*const (), F>(&ptr) }
            }
            _ => unreachable!("LuaUserdata method upval shape corrupted"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Vm::register_userdata
// ─────────────────────────────────────────────────────────────────────

impl Vm {
    /// Build (or fetch from cache) the metatable for `T`. Called
    /// lazily by [`Vm::create_userdata`] / [`Vm::set_userdata`];
    /// embedders rarely need to invoke it directly. Returns the same
    /// [`Gc<Table>`] on every call within a given `Vm` (keyed by
    /// `TypeId::of::<T>()`).
    ///
    /// The metatable is pinned as a host root so it survives GC even
    /// when no userdata of type `T` is currently reachable.
    pub fn register_userdata<T: LuaUserdata>(&mut self) -> Result<Gc<Table>, LuaError> {
        let tid = TypeId::of::<T>();
        if let Some(&mt) = self.userdata_metatables.get(&tid) {
            return Ok(mt);
        }
        let mut builder = MetatableBuilder::<T>::new(self);
        T::add_methods(&mut builder);
        let mt = builder.finalize()?;
        self.userdata_metatables.insert(tid, mt);
        // Pin as a host root so the cached metatable survives GC even
        // when no userdata of type T is reachable.
        self.pin_host(Value::Table(mt));
        Ok(mt)
    }
}
