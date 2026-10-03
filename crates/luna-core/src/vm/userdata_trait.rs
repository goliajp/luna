//! `LuaUserdata` trait sugar.
//!
//! Layered on top of host userdata (`UserdataPayload::Host`, `Vm::create_userdata`,
//! `Vm::userdata_borrow`), which lets embedders stash a `T: 'static`
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

use crate::runtime::heap::Gc;
use crate::runtime::table::Table;
use crate::runtime::value::Value;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::typed_native::{FromLuaArgs, IntoLuaReturn};

mod builder;
mod trampoline;
pub use builder::MetatableBuilder;

// ─────────────────────────────────────────────────────────────────────
// UserdataMarker — public facade over the GC marker passed to
// `LuaUserdata::trace`.
// ─────────────────────────────────────────────────────────────────────

/// Public facade over the GC mark accumulator passed to
/// [`LuaUserdata::trace`].
///
/// Wraps the crate-internal `Marker` (private GC primitive in
/// `runtime::heap`) so embedders never see the gray-stack / weak-table
/// internals. Holds a mutable
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
    pub fn mark<T: crate::runtime::GcObject>(&mut self, g: Gc<T>) -> bool {
        self.inner.mark(g)
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
/// for types written against v1.1.
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
/// Gc-bearing payloads are supported since v1.3: the trait has a default
/// [`trace`] method, and [`crate::runtime::userdata::UserdataPayload::Host`]
/// stores a monomorphic adapter for it.
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
    /// method; existing types remain source-compatible with
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
    /// True field-style `obj.name` (no parens) is
    /// supported alongside the legacy call-syntax `obj:name()` shape.
    /// When any `add_field_method_get` is registered, `MetatableBuilder`
    /// emits a native trampoline for `__index` that dispatches in the
    /// order *methods → field getters → nil*. Methods win on name
    /// collision (matches mlua and keeps v1.2 callers source-compatible).
    fn add_field_method_get<F, R>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &T) -> Result<R, LuaError> + Copy + 'static,
        R: IntoLuaReturn + 'static;

    /// Field-setter sugar: registers a setter for `obj.name = value`.
    /// When any `add_field_method_set` is registered,
    /// `MetatableBuilder` installs a `__newindex` trampoline that
    /// dispatches `(self, value)` to the registered setter. Unknown
    /// fields raise a runtime error rather than silently dropping the
    /// write.
    fn add_field_method_set<F, A>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Vm, &mut T, A) -> Result<(), LuaError> + Copy + 'static,
        A: FromLuaArgs + 'static;
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
