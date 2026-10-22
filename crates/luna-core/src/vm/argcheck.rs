//! PUC `lauxlib` argument checks — `luaL_check*`, `luaL_opt*`,
//! `luaL_typeerror` — for library natives.
//!
//! Every library native reads a typed argument through here, so a wrong or
//! missing argument raises the error the matching PUC version raises. The
//! rules differ by dialect and are reproduced as-is:
//!
//! - a missing argument is "no value", an explicit nil is "nil"
//!   (`lua_type` returns `LUA_TNONE` past the top);
//! - 5.3+ names a table or userdata by its `__name` metafield; ≤5.2 always
//!   uses the base type name;
//! - an integer argument given as a float or numeric string is truncated on
//!   ≤5.2 (`lua_number2integer` is a plain cast on every 64-bit target PUC
//!   builds for) and must be integral on 5.3+, which otherwise raises
//!   "number has no integer representation".
//!
//! Positions `i` are 0-based, as in [`Vm::nat_arg`]; messages number them
//! from 1.

use crate::numeric::{self, Num};
use crate::runtime::Value;
use crate::runtime::value::f2i_exact;
use crate::version::LuaVersion;
use crate::vm::builtins::arg_error;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
mod option;
pub(crate) use option::*;
mod stack;
pub(crate) use stack::*;
mod integer;
pub(crate) use integer::*;

/// A native's argument window: the callee slot and how many arguments the
/// caller passed.
#[derive(Clone, Copy)]
pub(crate) struct Args {
    pub fs: u32,
    pub n: u32,
}

impl Args {
    pub(crate) fn new(fs: u32, n: u32) -> Self {
        Args { fs, n }
    }

    /// `lua_type(L, i + 1) == LUA_TNONE`.
    pub(crate) fn is_none(self, i: u32) -> bool {
        i >= self.n
    }

    /// `lua_isnoneornil`.
    pub(crate) fn is_none_or_nil(self, vm: &Vm, i: u32) -> bool {
        self.is_none(i) || vm.nat_arg(self.fs, self.n, i).is_nil()
    }

    pub(crate) fn get(self, vm: &Vm, i: u32) -> Value {
        vm.nat_arg(self.fs, self.n, i)
    }
}

/// The type name `luaL_typeerror` reports for a present value.
pub(crate) fn typename_of(vm: &Vm, v: Value) -> String {
    if vm.version() >= LuaVersion::Lua53 {
        // `luaL_typeerror` singles out light userdata once `__name` fails
        match v {
            Value::LightUserdata(_) => "light userdata".to_string(),
            _ => vm.obj_typename(v),
        }
    } else {
        v.type_name().to_string()
    }
}

/// `luaL_typeerror`: "bad argument #n to 'f' (T expected, got U)".
pub(crate) fn type_error(vm: &mut Vm, a: Args, i: u32, expected: &str) -> LuaError {
    match type_error_msg(vm, a, i, expected) {
        Ok(m) => arg_error(vm, i + 1, &m),
        Err(e) => e,
    }
}

/// The "T expected, got U" `luaL_typeerror` pushes before `luaL_argerror`
/// adds to it; 5.3+ look a `__name` up first, which stays pushed when
/// present.
pub(crate) fn type_error_msg(
    vm: &mut Vm,
    a: Args,
    i: u32,
    expected: &str,
) -> Result<String, LuaError> {
    let v = (!a.is_none(i)).then(|| a.get(vm, i));
    value_type_msg(vm, v, expected)
}

/// [`type_error`] of `v`, argument `argno`.
pub(crate) fn value_type_error(vm: &mut Vm, v: Value, argno: u32, expected: &str) -> LuaError {
    match value_type_msg(vm, Some(v), expected) {
        Ok(m) => arg_error(vm, argno, &m),
        Err(e) => e,
    }
}

/// [`type_error_msg`] of `v` (`None`: no value).
fn value_type_msg(vm: &mut Vm, v: Option<Value>, expected: &str) -> Result<String, LuaError> {
    use crate::runtime::puc::fmt::FmtArg::S;
    let got = v.map_or_else(|| "no value".to_string(), |v| typename_of(vm, v));
    let v = v.filter(|_| vm.version() >= LuaVersion::Lua53);
    if let Some(v) = v {
        vm.metafield_check(v, b"__name")?;
    }
    let named = v.is_some_and(|v| !vm.get_mm(v, crate::vm::exec::Mm::Name).is_nil());
    vm.native_push(u32::from(named));
    let fmt = b"%s expected, got %s";
    option::pushed_extra(vm, 2, fmt, &[S(expected.as_bytes()), S(got.as_bytes())])
}

/// `luaL_checkany`.
pub(crate) fn check_any(vm: &mut Vm, a: Args, i: u32) -> Result<Value, LuaError> {
    if a.is_none(i) {
        return Err(arg_error(vm, i + 1, "value expected"));
    }
    Ok(a.get(vm, i))
}

/// `lua_tonumberx` on a value: numbers as-is, strings through the dialect's
/// string-to-number conversion, anything else `None`.
pub(crate) fn to_num(vm: &Vm, v: Value) -> Option<Num> {
    match v {
        Value::Int(x) => Some(Num::Int(x)),
        Value::Float(f) => Some(Num::Float(f)),
        Value::Str(s) => crate::vm::exec::str_to_num(s.as_bytes(), vm.version()),
        _ => None,
    }
}

/// `luaL_checknumber`.
pub(crate) fn check_number(vm: &mut Vm, a: Args, i: u32) -> Result<f64, LuaError> {
    match to_num(vm, a.get(vm, i)) {
        Some(n) if !a.is_none(i) => Ok(n.as_f64()),
        _ => Err(type_error(vm, a, i, "number")),
    }
}

/// `luaL_optnumber`.
pub(crate) fn opt_number(vm: &mut Vm, a: Args, i: u32, default: f64) -> Result<f64, LuaError> {
    if a.is_none_or_nil(vm, i) {
        return Ok(default);
    }
    check_number(vm, a, i)
}

/// `lua_tolstring` on a string or number: the bytes a library function sees.
/// A number is rendered the way the dialect prints it.
pub(crate) fn to_str_bytes(vm: &Vm, v: Value) -> Option<Vec<u8>> {
    match v {
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        Value::Int(x) => Some(numeric::num_to_string(Num::Int(x)).into_bytes()),
        Value::Float(f) => {
            Some(numeric::num_to_string_for(Num::Float(f), vm.float_fmt()).into_bytes())
        }
        _ => None,
    }
}

/// `luaL_argexpected(cond, i, tname)` from 5.4 on, and the plain
/// `luaL_argcheck(cond, i, "<tname> expected")` it replaced: 5.4+ appends
/// ", got <type>", earlier dialects do not.
pub(crate) fn arg_expected(vm: &mut Vm, a: Args, i: u32, tname: &str) -> LuaError {
    if vm.version() >= LuaVersion::Lua54 {
        type_error(vm, a, i, tname)
    } else {
        arg_error(vm, i + 1, &format!("{tname} expected"))
    }
}

/// `luaL_checklstring`, returning an interned string. A number argument is
/// converted with the dialect's rendering and, as `lua_tolstring` does,
/// the string replaces it in the argument slot, which keeps it alive for
/// as long as the native runs.
#[inline]
pub(crate) fn check_string(
    vm: &mut Vm,
    a: Args,
    i: u32,
) -> Result<crate::runtime::Gc<crate::runtime::LuaStr>, LuaError> {
    let v = a.get(vm, i);
    if let Value::Str(s) = v {
        return Ok(s);
    }
    check_string_conv(vm, a, i, v)
}

/// [`check_string`] for an argument that is not a string already: out of
/// line, so the common case pays nothing for the conversion.
#[inline(never)]
fn check_string_conv(
    vm: &mut Vm,
    a: Args,
    i: u32,
    v: Value,
) -> Result<crate::runtime::Gc<crate::runtime::LuaStr>, LuaError> {
    match to_str_bytes(vm, v) {
        Some(b) if !a.is_none(i) => {
            let s = vm.alloc_str(&b)?;
            vm.nat_set_arg(a.fs, i, Value::Str(s));
            // `lua_tolstring` checks for a collection after it converts
            vm.api_check_converted(Value::Str(s))?;
            Ok(s)
        }
        _ => Err(type_error(vm, a, i, "string")),
    }
}

/// `lua_tolstring` on argument `i`: a string as it is, a number converted
/// in place (as [`check_string`] converts it), anything else `None`.
pub(crate) fn tolstring_in_place(
    vm: &mut Vm,
    a: Args,
    i: u32,
) -> Result<Option<crate::runtime::Gc<crate::runtime::LuaStr>>, LuaError> {
    match a.get(vm, i) {
        Value::Str(s) => Ok(Some(s)),
        v @ (Value::Int(_) | Value::Float(_)) if !a.is_none(i) => {
            check_string_conv(vm, a, i, v).map(Some)
        }
        _ => Ok(None),
    }
}

/// `luaL_optlstring`: `None` when the argument is absent or nil.
pub(crate) fn opt_string(
    vm: &mut Vm,
    a: Args,
    i: u32,
) -> Result<Option<crate::runtime::Gc<crate::runtime::LuaStr>>, LuaError> {
    if a.is_none_or_nil(vm, i) {
        return Ok(None);
    }
    check_string(vm, a, i).map(Some)
}

/// `luaL_checktype(L, i + 1, LUA_TTABLE)`.
pub(crate) fn check_table(
    vm: &mut Vm,
    a: Args,
    i: u32,
) -> Result<crate::runtime::Gc<crate::runtime::Table>, LuaError> {
    match a.get(vm, i) {
        Value::Table(t) if !a.is_none(i) => Ok(t),
        _ => Err(type_error(vm, a, i, "table")),
    }
}

/// `luaL_checktype(L, i + 1, LUA_TFUNCTION)`.
pub(crate) fn check_function(vm: &mut Vm, a: Args, i: u32) -> Result<Value, LuaError> {
    match a.get(vm, i) {
        v @ (Value::Closure(_) | Value::Native(_)) if !a.is_none(i) => Ok(v),
        _ => Err(type_error(vm, a, i, "function")),
    }
}
