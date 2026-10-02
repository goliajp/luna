//! Native trampolines behind userdata methods, plus the upvalue pack helpers.

use super::LuaUserdata;
use crate::runtime::value::{NativeFn, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::typed_native::{FromLuaArgs, IntoLuaReturn};

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

/// `__index` trampoline. Installed by
/// [`MetatableBuilder::finalize`](super::MetatableBuilder::finalize) whenever any field getter is
/// registered. Upvals:
///
/// - `upvals[0]` — `Value::Table` (methods bucket) or `Value::Nil`
///   (field-only embedder).
/// - `upvals[1]` — `Value::Table` (field-getter dispatch table).
///
/// Args (PUC `__index` calling convention): `(self_userdata, key)`.
///
/// Dispatch order: methods → field getters → nil. Methods win on
/// collision; callers using `add_method("foo")` keep the existing
/// shape even if a same-named getter is registered later.
pub(super) fn index_trampoline(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let methods_upval = vm.nat_upval(fs, 0);
    let fields_upval = vm.nat_upval(fs, 1);
    let self_val = vm.nat_arg(fs, nargs, 0);
    let key = vm.nat_arg(fs, nargs, 1);

    // 1. methods first (preserves the plain-table precedence).
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

/// `__newindex` trampoline. Installed by
/// [`MetatableBuilder::finalize`](super::MetatableBuilder::finalize) whenever any field setter is
/// registered. Upvals:
///
/// - `upvals[0]` — `Value::Table` (field-setter dispatch table).
/// - `upvals[1]` — `Value::Str` (host type name, for error messages).
///
/// Args (PUC `__newindex` calling convention): `(self_userdata, key,
/// value)`. Unknown fields raise a runtime error rather than silently
/// dropping the write.
pub(super) fn newindex_trampoline(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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

pub(super) fn pack_method<T, F, A, R>(f: F) -> (NativeFn, Box<[Value]>)
where
    T: LuaUserdata,
    F: Fn(&mut Vm, &T, A) -> Result<R, LuaError> + Copy + 'static,
    A: FromLuaArgs + 'static,
    R: IntoLuaReturn + 'static,
{
    (method_trampoline::<T, F, A, R>, pack_zst_or_fnptr::<F>(f))
}

pub(super) fn pack_method_mut<T, F, A, R>(f: F) -> (NativeFn, Box<[Value]>)
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

pub(super) fn pack_function<F, A, R>(f: F) -> (NativeFn, Box<[Value]>)
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
