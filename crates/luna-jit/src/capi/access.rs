//! Access functions (stack -> C): type queries and conversions.

use super::*;

/// PUC `lua_type`: the `LUA_T*` tag at `idx`, `LUA_TNONE` where there is
/// no value.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_type`: the host does not
// link PUC's liblua next to this crate, which defines each `lua_*` symbol
// once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_type(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    api.get(idx).map_or(LUA_TNONE, type_tag)
}

/// `lua_type` for the C side.
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `luna_capi_type`; the C side
// is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_type(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    unsafe { lua_type(L, idx) }
}

/// PUC `lua_typename`: the name of type tag `tp`.
///
/// # Safety
/// `L` is a live thread of an open state.
// SAFETY: no other item in the link is named `lua_typename`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_typename(_L: *mut LuaState, tp: c_int) -> *const c_char {
    let name: &'static [u8] = match tp {
        LUA_TNIL => b"nil\0",
        LUA_TBOOLEAN => b"boolean\0",
        LUA_TLIGHTUSERDATA | LUA_TUSERDATA => b"userdata\0",
        LUA_TNUMBER => b"number\0",
        LUA_TSTRING => b"string\0",
        LUA_TTABLE => b"table\0",
        LUA_TFUNCTION => b"function\0",
        LUA_TTHREAD => b"thread\0",
        _ => b"no value\0",
    };
    name.as_ptr().cast()
}

/// The number a value converts to (PUC `tonumber`): numbers, and strings
/// the dialect reads as one.
pub(super) fn to_number(api: &Api, v: Value) -> Option<Value> {
    match v {
        Value::Int(_) | Value::Float(_) => Some(v),
        Value::Str(s) => api.vm.host_str_to_number(s.as_bytes()),
        _ => None,
    }
}

/// C's conversion of a double to a 64-bit integer, as the target's
/// instruction does it where C leaves it undefined (PUC ≤5.2
/// `lua_number2integer` is a plain cast).
fn c_cast_i64(f: f64) -> i64 {
    let in_range = (i64::MIN as f64..-(i64::MIN as f64)).contains(&f);
    if in_range {
        return f as i64;
    }
    if cfg!(any(target_arch = "x86_64", target_arch = "x86")) {
        i64::MIN
    } else if cfg!(any(target_arch = "riscv64", target_arch = "riscv32")) && f.is_nan() {
        i64::MAX
    } else {
        f as i64
    }
}

/// The integer a value converts to (PUC `lua_tointegerx`): up to 5.2 any
/// number, cut toward zero; from 5.3 on only a number with an exact
/// integer value.
pub(super) fn to_integer(api: &Api, v: Value) -> Option<i64> {
    match to_number(api, v)? {
        Value::Int(i) => Some(i),
        Value::Float(f) if api.version() <= LuaVersion::Lua52 => Some(c_cast_i64(f)),
        Value::Float(f) => {
            let in_range = (i64::MIN as f64..-(i64::MIN as f64)).contains(&f);
            (f.floor() == f && in_range).then_some(f as i64)
        }
        _ => None,
    }
}

/// PUC `lua_isnumber`.
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_isnumber`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isnumber(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    c_int::from(to_number(&api, v).is_some())
}

/// PUC `lua_isstring`: a string or a number.
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_isstring`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isstring(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    c_int::from(matches!(
        api.get(idx),
        Some(Value::Str(_) | Value::Int(_) | Value::Float(_))
    ))
}

/// PUC `lua_iscfunction`: a function that is not a Lua function.
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_iscfunction`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_iscfunction(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    c_int::from(matches!(api.get(idx), Some(Value::Native(_))))
}

/// PUC `lua_isinteger` (5.3+): an integer-typed number.
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_isinteger`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isinteger(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    c_int::from(matches!(api.get(idx), Some(Value::Int(_))))
}

/// PUC `lua_isuserdata`: a full or light userdata.
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_isuserdata`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isuserdata(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    c_int::from(matches!(
        api.get(idx),
        Some(Value::Userdata(_) | Value::LightUserdata(_))
    ))
}

/// Write `flag` through `out` unless it is null.
///
/// # Safety
/// `out` is null or writable.
unsafe fn set_flag(out: *mut c_int, flag: bool) {
    if !out.is_null() {
        // SAFETY: `out` is non-null and writable (# Safety)
        unsafe { *out = c_int::from(flag) };
    }
}

/// PUC `lua_tonumberx`: the number at `idx`, or 0; `*isnum` says which.
///
/// # Safety
/// As [`lua_type`]; `isnum` is null or writable.
// SAFETY: no other item in the link is named `lua_tonumberx`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tonumberx(L: *mut LuaState, idx: c_int, isnum: *mut c_int) -> f64 {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    let n = match to_number(&api, v) {
        Some(Value::Int(i)) => Some(i as f64),
        Some(Value::Float(f)) => Some(f),
        _ => None,
    };
    // SAFETY: `isnum` is null or writable (# Safety)
    unsafe { set_flag(isnum, n.is_some()) };
    n.unwrap_or(0.0)
}

/// PUC 5.1 `lua_tonumber` (a macro over `lua_tonumberx` from 5.2 on).
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_tonumber`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tonumber(L: *mut LuaState, idx: c_int) -> f64 {
    // SAFETY: the caller's contract (# Safety)
    unsafe { lua_tonumberx(L, idx, std::ptr::null_mut()) }
}

/// PUC `lua_tointegerx`: the integer at `idx`, or 0; `*isnum` says which.
///
/// # Safety
/// As [`lua_type`]; `isnum` is null or writable.
// SAFETY: no other item in the link is named `lua_tointegerx`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tointegerx(L: *mut LuaState, idx: c_int, isnum: *mut c_int) -> i64 {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    let n = to_integer(&api, v);
    // SAFETY: `isnum` is null or writable (# Safety)
    unsafe { set_flag(isnum, n.is_some()) };
    n.unwrap_or(0)
}

/// PUC 5.1 `lua_tointeger` (a macro from 5.2 on).
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_tointeger`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tointeger(L: *mut LuaState, idx: c_int) -> i64 {
    // SAFETY: the caller's contract (# Safety)
    unsafe { lua_tointegerx(L, idx, std::ptr::null_mut()) }
}

/// PUC 5.2 `lua_tounsignedx`: the number at `idx` modulo 2^32.
///
/// # Safety
/// As [`lua_type`]; `isnum` is null or writable.
// SAFETY: no other item in the link is named `lua_tounsignedx`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tounsignedx(L: *mut LuaState, idx: c_int, isnum: *mut c_int) -> u32 {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    let n = match to_number(&api, v) {
        Some(Value::Int(i)) => Some(i as u32),
        Some(Value::Float(f)) => {
            let sup = 4_294_967_296.0;
            Some((f - (f / sup).floor() * sup) as u32)
        }
        _ => None,
    };
    // SAFETY: `isnum` is null or writable (# Safety)
    unsafe { set_flag(isnum, n.is_some()) };
    n.unwrap_or(0)
}

/// PUC `lua_toboolean`.
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_toboolean`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_toboolean(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    c_int::from(api.get(idx).is_some_and(|v| v.truthy()))
}

/// `lua_tolstring`'s work: a number at `idx` is converted in place.
pub(super) fn tolstring(api: &mut Api, idx: c_int) -> Option<(*const c_char, usize)> {
    let s = match api.get(idx)? {
        Value::Str(s) => s,
        v @ (Value::Int(_) | Value::Float(_)) => {
            let text = api.vm.host_basic_text(v);
            let s = api.vm.heap.intern(&text);
            api.set(idx, Value::Str(s));
            s
        }
        _ => return None,
    };
    Some((s.as_c_ptr(), s.len()))
}

/// PUC `lua_tolstring`: the string at `idx` (a number there becomes one),
/// valid while the value stays on the stack; null for other values.
///
/// # Safety
/// As [`lua_type`]; `len` is null or writable.
// SAFETY: no other item in the link is named `lua_tolstring`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tolstring(
    L: *mut LuaState,
    idx: c_int,
    len: *mut usize,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let r = tolstring(&mut api, idx);
    if !len.is_null() {
        // SAFETY: `len` is non-null and writable (# Safety)
        unsafe { *len = r.map_or(0, |(_, n)| n) };
    }
    r.map_or(std::ptr::null(), |(p, _)| p)
}

/// `lua_tolstring` for the C side.
///
/// # Safety
/// As [`lua_tolstring`].
// SAFETY: no other item in the link is named `luna_capi_tolstring`; the C
// side is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_tolstring(
    L: *mut LuaState,
    idx: c_int,
    len: *mut usize,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    unsafe { lua_tolstring(L, idx, len) }
}

/// `lua_tolstring(L, idx, NULL)`: the headers make `lua_tostring` the macro
/// PUC has; this is for callers that bind it by name.
///
/// # Safety
/// As [`lua_type`].
// SAFETY: no other item in the link is named `lua_tostring`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tostring(L: *mut LuaState, idx: c_int) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    unsafe { lua_tolstring(L, idx, std::ptr::null_mut()) }
}
