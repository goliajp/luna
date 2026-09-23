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

/// The type name `luaL_typeerror` reports for argument `i`.
pub(crate) fn typename_at(vm: &Vm, a: Args, i: u32) -> String {
    if a.is_none(i) {
        return "no value".to_string();
    }
    let v = a.get(vm, i);
    if vm.version() >= LuaVersion::Lua53 {
        vm.obj_typename(v)
    } else {
        v.type_name().to_string()
    }
}

/// `luaL_typeerror`: "bad argument #n to 'f' (T expected, got U)".
pub(crate) fn type_error(vm: &mut Vm, a: Args, i: u32, expected: &str) -> LuaError {
    let got = typename_at(vm, a, i);
    arg_error(vm, i + 1, &format!("{expected} expected, got {got}"))
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
        Value::Str(s) => {
            let int_ok = vm.version() >= LuaVersion::Lua53;
            numeric::str2num(s.as_bytes(), int_ok, true)
        }
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

/// `luaL_checkinteger`.
pub(crate) fn check_integer(vm: &mut Vm, a: Args, i: u32) -> Result<i64, LuaError> {
    if let Value::Int(x) = a.get(vm, i) {
        return Ok(x);
    }
    let n = match to_num(vm, a.get(vm, i)) {
        Some(n) if !a.is_none(i) => n,
        _ => return Err(type_error(vm, a, i, "number")),
    };
    match n {
        Num::Int(x) => Ok(x),
        Num::Float(f) if vm.version() <= LuaVersion::Lua52 => Ok(f as i64),
        Num::Float(f) => {
            f2i_exact(f).ok_or_else(|| arg_error(vm, i + 1, "number has no integer representation"))
        }
    }
}

/// `luaL_optinteger`.
pub(crate) fn opt_integer(vm: &mut Vm, a: Args, i: u32, default: i64) -> Result<i64, LuaError> {
    if a.is_none_or_nil(vm, i) {
        return Ok(default);
    }
    check_integer(vm, a, i)
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

/// `luaL_checklstring`, returning an interned string. A number argument is
/// converted with the dialect's rendering.
pub(crate) fn check_string(
    vm: &mut Vm,
    a: Args,
    i: u32,
) -> Result<crate::runtime::Gc<crate::runtime::LuaStr>, LuaError> {
    let v = a.get(vm, i);
    if let Value::Str(s) = v {
        return Ok(s);
    }
    match to_str_bytes(vm, v) {
        Some(b) if !a.is_none(i) => Ok(vm.heap.intern(&b)),
        _ => Err(type_error(vm, a, i, "string")),
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
