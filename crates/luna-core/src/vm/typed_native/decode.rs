//! Argument decoding: `FromLuaValue` for single values, `FromLuaArgs` for argument tuples.

use crate::runtime::value::Value;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

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
/// (typed Rust native function trampoline).
pub trait FromLuaArgs: Sized {
    /// Decode `nargs` consecutive arguments starting at index `0` into `Self`.
    fn from_lua_args(vm: &mut Vm, fs: u32, nargs: u32) -> Result<Self, LuaError>;

    /// Decode `nargs - 1` arguments starting at index `1` — i.e. the
    /// `obj:method(args)` shape where slot `0` is the receiver and
    /// `args` start at slot `1`. `LuaUserdata` method
    /// trampolines call this; regular [`Vm::native_typed`] callers use
    /// [`from_lua_args`](Self::from_lua_args).
    fn from_lua_args_skip_self(vm: &mut Vm, fs: u32, nargs: u32) -> Result<Self, LuaError>;
}

impl FromLuaArgs for () {
    #[inline]
    fn from_lua_args(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<Self, LuaError> {
        Ok(())
    }
    #[inline]
    fn from_lua_args_skip_self(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<Self, LuaError> {
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
                #[inline]
                fn from_lua_args_skip_self(
                    vm: &mut Vm,
                    fs: u32,
                    nargs: u32,
                ) -> Result<Self, LuaError> {
                    Ok((
                        $(
                            $name::from_lua_value(vm.nat_arg(fs, nargs, $idx + 1))?,
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

/// Variadic decoder: collect **all** positional args into a
/// `Vec<Value>`. Useful for variadic natives (`redis.call(cmd, ...)`,
/// dispatch tables, etc.) where fixed-arity tuples would force an
/// artificial cap.
impl FromLuaArgs for Vec<Value> {
    #[inline]
    fn from_lua_args(vm: &mut Vm, fs: u32, nargs: u32) -> Result<Self, LuaError> {
        let mut out = Vec::with_capacity(nargs as usize);
        for i in 0..nargs {
            out.push(vm.nat_arg(fs, nargs, i));
        }
        Ok(out)
    }
    #[inline]
    fn from_lua_args_skip_self(vm: &mut Vm, fs: u32, nargs: u32) -> Result<Self, LuaError> {
        if nargs <= 1 {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity((nargs - 1) as usize);
        for i in 1..nargs {
            out.push(vm.nat_arg(fs, nargs, i));
        }
        Ok(out)
    }
}
