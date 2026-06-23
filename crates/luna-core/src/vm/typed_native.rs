//! `Vm::native_typed` + supporting traits (B5, Phase 2 P2-C).
//!
//! Embedders write typed Rust functions that look like Lua callables.
//! The framework decodes Lua arguments via [`FromLuaArgs`] (built on
//! per-argument [`FromLuaValue`]), invokes the typed fn, then encodes
//! the return via [`IntoLuaReturn`].
//!
//! ```
//! use luna_core::vm::Vm;
//! use luna_core::version::LuaVersion;
//! use luna_core::runtime::Value;
//!
//! let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
//! let add = vm.native_typed(|a: i64, b: i64| -> i64 { a + b });
//! vm.set_global("add", add).unwrap();
//! let r = vm.eval("return add(40, 2)").unwrap();
//! assert!(matches!(r[0], Value::Int(42)));
//! ```
//!
//! ## Supported shapes
//!
//! - **Argument count**: 0 to 3 (4-6 land in a follow-on commit; the
//!   trampoline pattern is mechanical to extend).
//! - **Argument types**: any [`FromLuaValue`] impl — `i64`, `f64`,
//!   `bool`, `String`, `Vec<u8>`, `Value`, `Option<T>`.
//! - **Return**: any [`IntoLuaReturn`] impl — `()`, single value,
//!   tuple up to 6, or `Result<T, LuaError>` for fallible natives.
//!
//! `F` must be `Fn(...) -> Out + Copy + 'static`. Both fn pointers
//! and **non-capturing closures** qualify (the latter are ZST so we
//! reconstruct them in the trampoline). Capturing closures are not
//! supported in P2-C; embedders use `vm.native_with(...)` directly
//! with explicit upvals, or wait for B8 LuaUserdata (Phase 3).

use crate::runtime::value::{NativeFn, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::into_value::IntoValue;

// ─────────────────────────────────────────────────────────────────────
// FromLuaValue — single-value decoder
// ─────────────────────────────────────────────────────────────────────

/// Decode a `Value` into a typed Rust value. Strict — no implicit
/// coercions beyond what Lua itself does (an exactly-integral float
/// can stand in for an integer).
pub trait FromLuaValue: Sized {
    /// Decode a single Lua [`Value`] into `Self`. Returns a
    /// `LuaError("type mismatch …")` if the value's type does not match.
    fn from_lua_value(v: Value) -> Result<Self, LuaError>;
}

impl FromLuaValue for Value {
    #[inline]
    fn from_lua_value(v: Value) -> Result<Self, LuaError> {
        Ok(v)
    }
}

impl FromLuaValue for i64 {
    #[inline]
    fn from_lua_value(v: Value) -> Result<Self, LuaError> {
        match v {
            Value::Int(i) => Ok(i),
            Value::Float(f) if f.is_finite() && f.fract() == 0.0 && (f as i64) as f64 == f => {
                Ok(f as i64)
            }
            _ => Err(LuaError(Value::Nil)),
        }
    }
}

impl FromLuaValue for f64 {
    #[inline]
    fn from_lua_value(v: Value) -> Result<Self, LuaError> {
        match v {
            Value::Int(i) => Ok(i as f64),
            Value::Float(f) => Ok(f),
            _ => Err(LuaError(Value::Nil)),
        }
    }
}

impl FromLuaValue for bool {
    #[inline]
    fn from_lua_value(v: Value) -> Result<Self, LuaError> {
        match v {
            Value::Bool(b) => Ok(b),
            _ => Err(LuaError(Value::Nil)),
        }
    }
}

impl FromLuaValue for String {
    #[inline]
    fn from_lua_value(v: Value) -> Result<Self, LuaError> {
        match v {
            Value::Str(s) => match std::str::from_utf8(s.as_bytes()) {
                Ok(t) => Ok(t.to_owned()),
                Err(_) => Err(LuaError(Value::Nil)),
            },
            _ => Err(LuaError(Value::Nil)),
        }
    }
}

impl FromLuaValue for Vec<u8> {
    #[inline]
    fn from_lua_value(v: Value) -> Result<Self, LuaError> {
        match v {
            Value::Str(s) => Ok(s.as_bytes().to_vec()),
            _ => Err(LuaError(Value::Nil)),
        }
    }
}

impl<T: FromLuaValue> FromLuaValue for Option<T> {
    #[inline]
    fn from_lua_value(v: Value) -> Result<Self, LuaError> {
        match v {
            Value::Nil => Ok(None),
            other => T::from_lua_value(other).map(Some),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// FromLuaArgs — tuple-shaped argument decoder
// ─────────────────────────────────────────────────────────────────────

/// Decode a tuple of typed Rust values from the VM's stack arguments
/// (B5 — typed Rust native function trampoline).
pub trait FromLuaArgs: Sized {
    /// Decode `nargs` consecutive arguments starting at `fs+1` into `Self`.
    fn from_lua_args(vm: &mut Vm, fs: u32, nargs: u32) -> Result<Self, LuaError>;
}

impl FromLuaArgs for () {
    #[inline]
    fn from_lua_args(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<Self, LuaError> {
        Ok(())
    }
}

macro_rules! impl_from_lua_args_tuple {
    ( $( ($($name:ident: $idx:tt),+) ),+ $(,)? ) => {
        $(
            impl<$($name: FromLuaValue),+> FromLuaArgs for ($($name,)+) {
                #[inline]
                fn from_lua_args(vm: &mut Vm, fs: u32, nargs: u32) -> Result<Self, LuaError> {
                    Ok((
                        $(
                            $name::from_lua_value(vm.nat_arg(fs, nargs, $idx))?,
                        )+
                    ))
                }
            }
        )+
    };
}
impl_from_lua_args_tuple! {
    (T0: 0),
    (T0: 0, T1: 1),
    (T0: 0, T1: 1, T2: 2),
    (T0: 0, T1: 1, T2: 2, T3: 3),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5),
}

// ─────────────────────────────────────────────────────────────────────
// IntoLuaReturn — tuple-shaped return encoder
// ─────────────────────────────────────────────────────────────────────

/// Push a typed Rust value (or tuple of values) onto the VM's stack as a
/// native function's return values (B5).
pub trait IntoLuaReturn {
    /// Push the encoded values starting at `fs` and return the result count.
    fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError>;
}

impl IntoLuaReturn for () {
    #[inline]
    fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
        Ok(vm.nat_return(fs, &[]))
    }
}

impl<Out: IntoLuaReturn> IntoLuaReturn for Result<Out, LuaError> {
    #[inline]
    fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
        self?.into_lua_return(vm, fs)
    }
}

macro_rules! impl_into_lua_return_single {
    ($($t:ty),+ $(,)?) => {
        $(
            impl IntoLuaReturn for $t {
                #[inline]
                fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
                    let v = <$t as IntoValue>::into_value(self, vm);
                    Ok(vm.nat_return(fs, &[v]))
                }
            }
        )+
    };
}
impl_into_lua_return_single!(
    Value,
    i64,
    i32,
    i16,
    i8,
    u32,
    u16,
    u8,
    f64,
    f32,
    bool,
    String,
    Vec<u8>,
);

impl IntoLuaReturn for &'static str {
    #[inline]
    fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
        let v = self.into_value(vm);
        Ok(vm.nat_return(fs, &[v]))
    }
}

macro_rules! impl_into_lua_return_tuple {
    ( $( ($($name:ident: $idx:tt),+) ),+ $(,)? ) => {
        $(
            impl<$($name: IntoValue),+> IntoLuaReturn for ($($name,)+) {
                #[inline]
                fn into_lua_return(self, vm: &mut Vm, fs: u32) -> Result<u32, LuaError> {
                    let vs = [
                        $( self.$idx.into_value(vm), )+
                    ];
                    Ok(vm.nat_return(fs, &vs))
                }
            }
        )+
    };
}
impl_into_lua_return_tuple! {
    (T0: 0, T1: 1),
    (T0: 0, T1: 1, T2: 2),
    (T0: 0, T1: 1, T2: 2, T3: 3),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4),
    (T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5),
}

// ─────────────────────────────────────────────────────────────────────
// NativeTypedSig — uniform trampoline for fn pointers + ZST closures
// ─────────────────────────────────────────────────────────────────────

/// Marker types encoding the arity of an `Fn(...)` callable. Used as
/// the second type parameter of [`NativeTypedSig`] so multiple blanket
/// `impl<F, Marker> NativeTypedSig<Marker> for F` arms don't overlap
/// from the compiler's coherence perspective. Embedders never name
/// these — the compiler infers from the closure signature.
pub struct Arity0;
/// Marker for a 1-argument callable.
pub struct Arity1<In0>(std::marker::PhantomData<In0>);
/// Marker for a 2-argument callable.
pub struct Arity2<In0, In1>(std::marker::PhantomData<(In0, In1)>);
/// Marker for a 3-argument callable.
pub struct Arity3<In0, In1, In2>(std::marker::PhantomData<(In0, In1, In2)>);
/// Marker for a 4-argument callable.
pub struct Arity4<In0, In1, In2, In3>(std::marker::PhantomData<(In0, In1, In2, In3)>);
/// Marker for a 5-argument callable.
pub struct Arity5<In0, In1, In2, In3, In4>(std::marker::PhantomData<(In0, In1, In2, In3, In4)>);
/// Marker for a 6-argument callable.
pub struct Arity6<In0, In1, In2, In3, In4, In5>(
    std::marker::PhantomData<(In0, In1, In2, In3, In4, In5)>,
);

/// Convert a typed callable into the `(erased NativeFn, upvals)` shape
/// the dispatcher consumes. The `Marker` type parameter encodes the
/// callable's arity so impls for `Fn()`, `Fn(In0)`, etc. coexist
/// without coherence conflicts.
///
/// **Storage discrimination** (constant-folded at monomorphization):
/// - If `F` is zero-sized (ZST closure, fn item): upvals empty;
///   trampoline reconstructs `F` via `MaybeUninit::uninit().assume_init()`.
/// - If `F` is pointer-sized (`fn` pointer): stored as
///   `Value::LightUserdata` in `upvals[0]`; trampoline transmutes back.
/// - Other sizes (capturing closures): runtime panic via `assert!` in
///   `pack` — embedder must use `vm.native_with(...)` directly.
pub trait NativeTypedSig<Marker> {
    /// Convert the callable into the `(NativeFn, upvals)` pair the
    /// dispatcher consumes.
    fn into_native(self) -> (NativeFn, Box<[Value]>);
}

#[inline]
fn reconstruct<F: Copy + 'static>(vm: &Vm, fs: u32) -> F {
    if std::mem::size_of::<F>() == 0 {
        // SAFETY: F is a ZST. MaybeUninit::<F>::uninit().assume_init()
        // returns a valid F because there are no bytes to initialize.
        unsafe { std::mem::MaybeUninit::<F>::uninit().assume_init() }
    } else {
        let upval = vm.nat_upval(fs, 0);
        match upval {
            Value::LightUserdata(ptr) => {
                debug_assert_eq!(
                    std::mem::size_of::<F>(),
                    std::mem::size_of::<*const ()>(),
                    "non-ZST F must be fn-pointer-sized"
                );
                // SAFETY: stored via `into_native` below with the
                // same F. The NativeClosure's upvals are immutable
                // after construction.
                unsafe { std::mem::transmute_copy::<*const (), F>(&ptr) }
            }
            _ => unreachable!("native_typed upval shape corrupted"),
        }
    }
}

#[inline]
fn pack<F: Copy + 'static>(f: F) -> Box<[Value]> {
    if std::mem::size_of::<F>() == 0 {
        Box::new([])
    } else {
        assert!(
            std::mem::size_of::<F>() == std::mem::size_of::<*const ()>(),
            "native_typed: F must be ZST (non-capturing closure / fn item) or fn pointer; \
             capturing closures unsupported (use vm.native_with directly)"
        );
        // SAFETY: F is fn-pointer-sized; transmute_copy reads its
        // bytes as a raw *const () for storage. Recovered in
        // `reconstruct` above.
        let raw_ptr: *const () = unsafe { std::mem::transmute_copy(&f) };
        Box::new([Value::LightUserdata(raw_ptr)])
    }
}

// Arity 0
impl<F, Out> NativeTypedSig<(Arity0, Out)> for F
where
    F: Fn() -> Out + Copy + 'static,
    Out: IntoLuaReturn + 'static,
{
    fn into_native(self) -> (NativeFn, Box<[Value]>) {
        fn trampoline<F: Fn() -> Out + Copy + 'static, Out: IntoLuaReturn + 'static>(
            vm: &mut Vm,
            fs: u32,
            _nargs: u32,
        ) -> Result<u32, LuaError> {
            let f: F = reconstruct(vm, fs);
            f().into_lua_return(vm, fs)
        }
        (trampoline::<F, Out>, pack(self))
    }
}

// Arity 1
impl<F, In0, Out> NativeTypedSig<(Arity1<In0>, Out)> for F
where
    F: Fn(In0) -> Out + Copy + 'static,
    In0: FromLuaValue + 'static,
    Out: IntoLuaReturn + 'static,
{
    fn into_native(self) -> (NativeFn, Box<[Value]>) {
        fn trampoline<
            F: Fn(In0) -> Out + Copy + 'static,
            In0: FromLuaValue + 'static,
            Out: IntoLuaReturn + 'static,
        >(
            vm: &mut Vm,
            fs: u32,
            nargs: u32,
        ) -> Result<u32, LuaError> {
            let f: F = reconstruct(vm, fs);
            let (a0,) = <(In0,) as FromLuaArgs>::from_lua_args(vm, fs, nargs)?;
            f(a0).into_lua_return(vm, fs)
        }
        (trampoline::<F, In0, Out>, pack(self))
    }
}

// Arity 2
impl<F, In0, In1, Out> NativeTypedSig<(Arity2<In0, In1>, Out)> for F
where
    F: Fn(In0, In1) -> Out + Copy + 'static,
    In0: FromLuaValue + 'static,
    In1: FromLuaValue + 'static,
    Out: IntoLuaReturn + 'static,
{
    fn into_native(self) -> (NativeFn, Box<[Value]>) {
        fn trampoline<
            F: Fn(In0, In1) -> Out + Copy + 'static,
            In0: FromLuaValue + 'static,
            In1: FromLuaValue + 'static,
            Out: IntoLuaReturn + 'static,
        >(
            vm: &mut Vm,
            fs: u32,
            nargs: u32,
        ) -> Result<u32, LuaError> {
            let f: F = reconstruct(vm, fs);
            let (a0, a1) =
                <(In0, In1) as FromLuaArgs>::from_lua_args(vm, fs, nargs)?;
            f(a0, a1).into_lua_return(vm, fs)
        }
        (trampoline::<F, In0, In1, Out>, pack(self))
    }
}

// Arity 3
impl<F, In0, In1, In2, Out> NativeTypedSig<(Arity3<In0, In1, In2>, Out)> for F
where
    F: Fn(In0, In1, In2) -> Out + Copy + 'static,
    In0: FromLuaValue + 'static,
    In1: FromLuaValue + 'static,
    In2: FromLuaValue + 'static,
    Out: IntoLuaReturn + 'static,
{
    fn into_native(self) -> (NativeFn, Box<[Value]>) {
        fn trampoline<
            F: Fn(In0, In1, In2) -> Out + Copy + 'static,
            In0: FromLuaValue + 'static,
            In1: FromLuaValue + 'static,
            In2: FromLuaValue + 'static,
            Out: IntoLuaReturn + 'static,
        >(
            vm: &mut Vm,
            fs: u32,
            nargs: u32,
        ) -> Result<u32, LuaError> {
            let f: F = reconstruct(vm, fs);
            let (a0, a1, a2) =
                <(In0, In1, In2) as FromLuaArgs>::from_lua_args(vm, fs, nargs)?;
            f(a0, a1, a2).into_lua_return(vm, fs)
        }
        (trampoline::<F, In0, In1, In2, Out>, pack(self))
    }
}

// Arity 4
impl<F, In0, In1, In2, In3, Out> NativeTypedSig<(Arity4<In0, In1, In2, In3>, Out)> for F
where
    F: Fn(In0, In1, In2, In3) -> Out + Copy + 'static,
    In0: FromLuaValue + 'static,
    In1: FromLuaValue + 'static,
    In2: FromLuaValue + 'static,
    In3: FromLuaValue + 'static,
    Out: IntoLuaReturn + 'static,
{
    fn into_native(self) -> (NativeFn, Box<[Value]>) {
        fn trampoline<
            F: Fn(In0, In1, In2, In3) -> Out + Copy + 'static,
            In0: FromLuaValue + 'static,
            In1: FromLuaValue + 'static,
            In2: FromLuaValue + 'static,
            In3: FromLuaValue + 'static,
            Out: IntoLuaReturn + 'static,
        >(
            vm: &mut Vm,
            fs: u32,
            nargs: u32,
        ) -> Result<u32, LuaError> {
            let f: F = reconstruct(vm, fs);
            let (a0, a1, a2, a3) =
                <(In0, In1, In2, In3) as FromLuaArgs>::from_lua_args(vm, fs, nargs)?;
            f(a0, a1, a2, a3).into_lua_return(vm, fs)
        }
        (trampoline::<F, In0, In1, In2, In3, Out>, pack(self))
    }
}

// Arity 5
impl<F, In0, In1, In2, In3, In4, Out> NativeTypedSig<(Arity5<In0, In1, In2, In3, In4>, Out)> for F
where
    F: Fn(In0, In1, In2, In3, In4) -> Out + Copy + 'static,
    In0: FromLuaValue + 'static,
    In1: FromLuaValue + 'static,
    In2: FromLuaValue + 'static,
    In3: FromLuaValue + 'static,
    In4: FromLuaValue + 'static,
    Out: IntoLuaReturn + 'static,
{
    fn into_native(self) -> (NativeFn, Box<[Value]>) {
        fn trampoline<
            F: Fn(In0, In1, In2, In3, In4) -> Out + Copy + 'static,
            In0: FromLuaValue + 'static,
            In1: FromLuaValue + 'static,
            In2: FromLuaValue + 'static,
            In3: FromLuaValue + 'static,
            In4: FromLuaValue + 'static,
            Out: IntoLuaReturn + 'static,
        >(
            vm: &mut Vm,
            fs: u32,
            nargs: u32,
        ) -> Result<u32, LuaError> {
            let f: F = reconstruct(vm, fs);
            let (a0, a1, a2, a3, a4) =
                <(In0, In1, In2, In3, In4) as FromLuaArgs>::from_lua_args(vm, fs, nargs)?;
            f(a0, a1, a2, a3, a4).into_lua_return(vm, fs)
        }
        (
            trampoline::<F, In0, In1, In2, In3, In4, Out>,
            pack(self),
        )
    }
}

// Arity 6
impl<F, In0, In1, In2, In3, In4, In5, Out>
    NativeTypedSig<(Arity6<In0, In1, In2, In3, In4, In5>, Out)> for F
where
    F: Fn(In0, In1, In2, In3, In4, In5) -> Out + Copy + 'static,
    In0: FromLuaValue + 'static,
    In1: FromLuaValue + 'static,
    In2: FromLuaValue + 'static,
    In3: FromLuaValue + 'static,
    In4: FromLuaValue + 'static,
    In5: FromLuaValue + 'static,
    Out: IntoLuaReturn + 'static,
{
    fn into_native(self) -> (NativeFn, Box<[Value]>) {
        fn trampoline<
            F: Fn(In0, In1, In2, In3, In4, In5) -> Out + Copy + 'static,
            In0: FromLuaValue + 'static,
            In1: FromLuaValue + 'static,
            In2: FromLuaValue + 'static,
            In3: FromLuaValue + 'static,
            In4: FromLuaValue + 'static,
            In5: FromLuaValue + 'static,
            Out: IntoLuaReturn + 'static,
        >(
            vm: &mut Vm,
            fs: u32,
            nargs: u32,
        ) -> Result<u32, LuaError> {
            let f: F = reconstruct(vm, fs);
            let (a0, a1, a2, a3, a4, a5) =
                <(In0, In1, In2, In3, In4, In5) as FromLuaArgs>::from_lua_args(vm, fs, nargs)?;
            f(a0, a1, a2, a3, a4, a5).into_lua_return(vm, fs)
        }
        (
            trampoline::<F, In0, In1, In2, In3, In4, In5, Out>,
            pack(self),
        )
    }
}

// ─────────────────────────────────────────────────────────────────────
// Vm::native_typed
// ─────────────────────────────────────────────────────────────────────

impl Vm {
    /// Register a typed Rust function as a Lua-callable `Value`. The
    /// callable must be a fn-pointer or a non-capturing closure
    /// (`Copy + 'static + ZST or fn-pointer-sized`). For capturing
    /// closures use `vm.native_with(...)` with explicit upvals.
    ///
    /// ```
    /// # use luna_core::vm::Vm;
    /// # use luna_core::version::LuaVersion;
    /// # use luna_core::runtime::Value;
    /// # let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    /// let add = vm.native_typed(|a: i64, b: i64| -> i64 { a + b });
    /// vm.set_global("add", add).unwrap();
    /// let r = vm.eval("return add(40, 2)").unwrap();
    /// assert!(matches!(r[0], Value::Int(42)));
    /// ```
    pub fn native_typed<F, Marker>(&mut self, f: F) -> Value
    where
        F: NativeTypedSig<Marker>,
    {
        let (raw_fn, upvals) = f.into_native();
        self.native_with(raw_fn, upvals)
    }
}
