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
    /// **v1.3 (UD1+UD2)**: true field-style `obj.name` (no parens) is
    /// supported alongside the legacy call-syntax `obj:name()` shape.
    /// When any `add_field_method_get` is registered, `MetatableBuilder`
    /// emits a native trampoline for `__index` that dispatches in the
    /// order *methods → field getters → nil*. Methods win on name
    /// collision (matches mlua and keeps v1.2 callers source-compatible).
    fn add_field_method_get<F, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &T) -> Result<R, LuaError> + Copy + 'static,
        R: IntoLuaReturn + 'static;

    /// Field-setter sugar: registers a setter for `obj.name = value`
    /// (v1.3 UD1). When any `add_field_method_set` is registered,
    /// `MetatableBuilder` installs a `__newindex` trampoline that
    /// dispatches `(self, value)` to the registered setter. Unknown
    /// fields raise a runtime error rather than silently dropping the
    /// write (matches `code/no-unsolicited-fallback`).
    fn add_field_method_set<F, A>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<(), LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static;
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
    /// Field getters for true field-style `obj.name` (v1.3 UD2).
    fields_get: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    /// Field setters for `obj.name = value` (v1.3 UD1).
    fields_set: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    /// Direct metatable entries (metamethods + static functions).
    meta_entries: Vec<(Gc<crate::runtime::LuaStr>, Value)>,
    _phantom: PhantomData<fn() -> T>,
}

impl<'vm, T: LuaUserdata> MetatableBuilder<'vm, T> {
    fn new(vm: &'vm mut Vm) -> Self {
        Self {
            vm,
            methods: Vec::new(),
            fields_get: Vec::new(),
            fields_set: Vec::new(),
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
    ///
    /// Three-way fork on `__index`:
    /// 1. **No methods, no field getters** → no `__index` slot.
    /// 2. **Methods only, no field getters** → v1.2 fast path:
    ///    `__index` is a plain `Value::Table` of methods.
    /// 3. **Any field getters registered** → `__index` is a native
    ///    trampoline ([`index_trampoline`]) with upvals
    ///    `(methods_table_or_nil, fields_get_table)` dispatching
    ///    *methods → field-getters → nil*.
    ///
    /// `__newindex` is installed only when any field setter is
    /// registered (Phase UD1).
    fn finalize(self) -> Result<Gc<Table>, LuaError> {
        let MetatableBuilder {
            vm,
            methods,
            fields_get,
            fields_set,
            meta_entries,
            ..
        } = self;

        let mt = vm.heap.new_table();
        // __name — drives PUC-style error messages.
        let name_key = vm.heap.intern(b"__name");
        let type_name_str = vm.heap.intern(T::type_name().as_bytes());
        let name_val = Value::Str(type_name_str);
        // SAFETY: mt is a fresh Gc<Table>; the heap is single-threaded.
        unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(name_key), name_val)?;

        // Helper: build a Gc<Table> from a (key, value) bucket (or None
        // for the empty case so the caller can skip the allocation).
        let mk_bucket = |vm: &mut Vm,
                         entries: Vec<(Gc<crate::runtime::LuaStr>, Value)>|
         -> Result<Option<Gc<Table>>, LuaError> {
            if entries.is_empty() {
                return Ok(None);
            }
            let t = vm.heap.new_table();
            for (k, v) in entries {
                // SAFETY: t is freshly allocated.
                unsafe { t.as_mut() }.set(&mut vm.heap, Value::Str(k), v)?;
            }
            Ok(Some(t))
        };

        // __index — fork on whether any field getters are registered.
        if fields_get.is_empty() {
            // Methods-only fast path (v1.2 shape preserved).
            if let Some(idx) = mk_bucket(vm, methods)? {
                let key = vm.heap.intern(b"__index");
                // SAFETY: mt is freshly allocated.
                unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(key), Value::Table(idx))?;
            }
        } else {
            // Trampoline path — methods table + fields_get table as upvals.
            let methods_val = match mk_bucket(vm, methods)? {
                Some(t) => Value::Table(t),
                None => Value::Nil,
            };
            let fields_val =
                Value::Table(mk_bucket(vm, fields_get)?.expect("fields_get non-empty checked"));
            let upvals: Box<[Value]> = Box::new([methods_val, fields_val]);
            let trampoline = vm.native_with(index_trampoline, upvals);
            let key = vm.heap.intern(b"__index");
            // SAFETY: mt is freshly allocated.
            unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(key), trampoline)?;
        }

        // __newindex — installed only when any field setter is registered.
        if !fields_set.is_empty() {
            let setters_tbl = mk_bucket(vm, fields_set)?.expect("fields_set non-empty checked");
            let upvals: Box<[Value]> =
                Box::new([Value::Table(setters_tbl), Value::Str(type_name_str)]);
            let trampoline = vm.native_with(newindex_trampoline, upvals);
            let key = vm.heap.intern(b"__newindex");
            // SAFETY: mt is freshly allocated.
            unsafe { mt.as_mut() }.set(&mut vm.heap, Value::Str(key), trampoline)?;
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
        // v1.3 UD2: getter lives ONLY in the fields_get bucket. The
        // `__index` trampoline calls it with `(self,)` so `obj.name`
        // returns the field value directly.
        //
        // **Breaking change from v1.2**: the v1.2 call-syntax shape
        // (`obj:name()`) no longer works for getters defined this way
        // — the trampoline calls the getter and returns its value, so
        // `obj.name` is `Value::Int(...)` not the closure, and
        // `obj:name()` evaluates to `Int(...)(obj)` which errors.
        // Embedders who need both shapes should register an explicit
        // `add_method("name", ...)` (returns the closure unchanged
        // through the table-`__index` fallback) alongside the
        // `add_field_method_get` if a same-named field-getter is also
        // wanted. The audit's "dual registration" idea was
        // load-bearing-broken — see CHANGELOG [1.3.0] for migration.
        let (raw_fn, upvals) = pack_method::<T, _, (), R>(adapter);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.fields_get.push((k, v));
    }

    fn add_field_method_set<F, A>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<(), LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static,
    {
        // Same trampoline shape as add_method_mut — `()` is a valid
        // `IntoLuaReturn`. Native is bucketed into `fields_set`, which
        // `newindex_trampoline` forwards to.
        let (raw_fn, upvals) = pack_method_mut::<T, F, A, ()>(f);
        let v = self.make_native(raw_fn, upvals);
        let k = self.intern(name);
        self.fields_set.push((k, v));
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

/// `__index` trampoline (v1.3 UD2). Installed by
/// [`MetatableBuilder::finalize`] whenever any field getter is
/// registered. Upvals:
///
/// - `upvals[0]` — `Value::Table` (methods bucket) or `Value::Nil`
///   (field-only embedder).
/// - `upvals[1]` — `Value::Table` (field-getter dispatch table).
///
/// Args (PUC `__index` calling convention): `(self_userdata, key)`.
///
/// Dispatch order: methods → field getters → nil. Methods win on
/// collision; v1.2 callers using `add_method("foo")` keep the existing
/// shape even if a same-named getter is registered later.
fn index_trampoline(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let methods_upval = vm.nat_upval(fs, 0);
    let fields_upval = vm.nat_upval(fs, 1);
    let self_val = vm.nat_arg(fs, nargs, 0);
    let key = vm.nat_arg(fs, nargs, 1);

    // 1. methods first (preserves v1.2 precedence).
    if let Value::Table(m) = methods_upval {
        let v = m.get(key);
        if !v.is_nil() {
            return Ok(vm.nat_return(fs, &[v]));
        }
    }
    // 2. field getters — call getter(self,) and surface its result.
    if let Value::Table(g) = fields_upval {
        let getter = g.get(key);
        if !getter.is_nil() {
            let mut results = vm.call_value(getter, &[self_val])?;
            let r = if results.is_empty() {
                Value::Nil
            } else {
                results.swap_remove(0)
            };
            return Ok(vm.nat_return(fs, &[r]));
        }
    }
    // 3. nothing matched — return nil (matches PUC `__index` semantics).
    Ok(vm.nat_return(fs, &[Value::Nil]))
}

/// `__newindex` trampoline (v1.3 UD1). Installed by
/// [`MetatableBuilder::finalize`] whenever any field setter is
/// registered. Upvals:
///
/// - `upvals[0]` — `Value::Table` (field-setter dispatch table).
/// - `upvals[1]` — `Value::Str` (host type name, for error messages).
///
/// Args (PUC `__newindex` calling convention): `(self_userdata, key,
/// value)`. Unknown fields raise a runtime error rather than silently
/// dropping the write (matches `code/no-unsolicited-fallback`).
fn newindex_trampoline(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let setters_upval = vm.nat_upval(fs, 0);
    let type_name_upval = vm.nat_upval(fs, 1);
    let self_val = vm.nat_arg(fs, nargs, 0);
    let key = vm.nat_arg(fs, nargs, 1);
    let value = vm.nat_arg(fs, nargs, 2);

    if let Value::Table(s) = setters_upval {
        let setter = s.get(key);
        if !setter.is_nil() {
            // setter(self, value) → Result<(), LuaError>; discard return.
            vm.call_value(setter, &[self_val, value])?;
            return Ok(vm.nat_return(fs, &[]));
        }
    }
    // Unknown field — pretty-print key + host type name.
    let key_str = match key {
        Value::Str(s) => std::str::from_utf8(s.as_bytes())
            .unwrap_or("<non-utf8>")
            .to_string(),
        other => format!("{:?}", other),
    };
    let type_str = match type_name_upval {
        Value::Str(s) => std::str::from_utf8(s.as_bytes())
            .unwrap_or("<non-utf8>")
            .to_string(),
        _ => "userdata".to_string(),
    };
    Err(vm.rt_err(&format!(
        "attempt to write unknown field '{}' on {} (no setter registered)",
        key_str, type_str
    )))
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
