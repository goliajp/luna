//! `NativeTypedSig`: one trampoline per arity, for fn pointers and ZST closures.

use super::{FromLuaArgs, FromLuaValue, IntoLuaReturn};
use crate::runtime::value::{NativeFn, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

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
        #[allow(clippy::uninit_assumed_init)]
        // ZST-only branch guarded above; constant-folded at monomorphization.
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
            let (a0, a1) = <(In0, In1) as FromLuaArgs>::from_lua_args(vm, fs, nargs)?;
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
            let (a0, a1, a2) = <(In0, In1, In2) as FromLuaArgs>::from_lua_args(vm, fs, nargs)?;
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
        (trampoline::<F, In0, In1, In2, In3, In4, Out>, pack(self))
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
