//! `luaL_checkinteger` and its kin: the integer a library function sees.

use super::*;
use crate::vm::c_abi;

/// `luaL_checkinteger`.
#[inline]
pub(crate) fn check_integer(vm: &mut Vm, a: Args, i: u32) -> Result<i64, LuaError> {
    if let Value::Int(x) = a.get(vm, i) {
        // 5.1's and 5.2's number is a double, converted to their 32-bit
        // `lua_Integer` on a 32-bit target
        #[cfg(target_pointer_width = "32")]
        if vm.version() <= LuaVersion::Lua52 {
            return Ok(c_abi::number2integer(vm.version(), x as f64));
        }
        return Ok(x);
    }
    check_integer_conv(vm, a, i)
}

/// [`check_integer`] for an argument that is not an integer already: out
/// of line, so the common case pays nothing for the conversions.
#[inline(never)]
fn check_integer_conv(vm: &mut Vm, a: Args, i: u32) -> Result<i64, LuaError> {
    let n = match to_num(vm, a.get(vm, i)) {
        Some(n) if !a.is_none(i) => n,
        _ => return Err(type_error(vm, a, i, "number")),
    };
    match n {
        #[cfg(target_pointer_width = "32")]
        Num::Int(x) if vm.version() <= LuaVersion::Lua52 => {
            Ok(c_abi::number2integer(vm.version(), x as f64))
        }
        Num::Int(x) => Ok(x),
        Num::Float(f) if vm.version() <= LuaVersion::Lua52 => {
            Ok(c_abi::number2integer(vm.version(), f))
        }
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

/// 5.2 `luaL_checkunsigned`. `lua_Unsigned` is 32 bits there, and every
/// target PUC 5.2 builds for converts with `LUA_IEEE754TRICK`: add
/// 1.5 * 2^52 and keep the low word of the double. That rounds to nearest
/// (ties to even) and wraps modulo 2^32, so `3.5` becomes 4 and `-1`
/// becomes 0xFFFFFFFF.
pub(crate) fn check_unsigned52(vm: &mut Vm, a: Args, i: u32) -> Result<u32, LuaError> {
    match to_num(vm, a.get(vm, i)) {
        Some(n) if !a.is_none(i) => Ok(c_abi::ieee_trick(n.as_f64())),
        _ => Err(type_error(vm, a, i, "number")),
    }
}

/// `(int)luaL_checkinteger` — what `luaL_checkint` (≤5.2) and PUC's explicit
/// `(int)` casts (e.g. `error`'s level on 5.3+) hand the C code: the integer
/// truncated to 32 bits, as the cast does on every two's-complement target.
pub(crate) fn check_int(vm: &mut Vm, a: Args, i: u32) -> Result<i32, LuaError> {
    check_integer(vm, a, i).map(|x| x as i32)
}

/// `(int)luaL_optinteger` — see [`check_int`].
pub(crate) fn opt_int(vm: &mut Vm, a: Args, i: u32, default: i32) -> Result<i32, LuaError> {
    if a.is_none_or_nil(vm, i) {
        return Ok(default);
    }
    check_int(vm, a, i)
}
