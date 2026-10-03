//! Tables: writing fields with and without metamethods. The functions are
//! reached through their C wrappers (`csrc/shim_values.c`): a write can
//! raise (a metamethod's error, a nil or NaN key, a read-only table).

use super::tables::table_at;
use super::*;

/// `t[k] = v` with metamethods, then drop the top `n` values (the key and
/// value the caller left there while the write ran).
fn set_and_pop(api: &mut Api, t: Value, k: Value, v: Value, n: usize) {
    match api.vm.host_set_index(t, k, v) {
        Ok(()) => {
            let top = api.top();
            api.truncate(top - n);
        }
        Err(e) => api.raise(e),
    }
}

/// `t[k] = v` without metamethods into the table at `idx`, then drop the
/// top `n` values.
fn raw_set_and_pop(api: &mut Api, idx: c_int, k: Value, v: Value, n: usize) {
    let t = table_at(api, idx);
    match api.vm.host_raw_set(t, k, v) {
        Ok(()) => {
            let top = api.top();
            api.truncate(top - n);
        }
        Err(e) => api.raise(e),
    }
}

/// PUC `lua_settable`: `t[key] = value` (`t` at `idx`), the value on top
/// and the key below it, through `__newindex`; pops both.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_settable`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_settable(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = api.get_or_nil(idx);
    let k = api.get_or_nil(-2);
    let v = api.get_or_nil(-1);
    set_and_pop(&mut api, t, k, v, 2);
}

/// PUC `lua_setfield`: `t[k] = value` (`t` at `idx`), the value on top,
/// through `__newindex`; pops the value.
///
/// # Safety
/// As [`luna_capi_lua_settable`]; `k` is a NUL-terminated string.
// SAFETY: no other item in the link is named `luna_capi_lua_setfield`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_setfield(L: *mut LuaState, idx: c_int, k: *const c_char) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = api.get_or_nil(idx);
    let v = api.get_or_nil(-1);
    // SAFETY: `k` is NUL-terminated (# Safety)
    let key = api.str(unsafe { c_bytes(k) }.unwrap_or_default());
    api.push(key);
    set_and_pop(&mut api, t, key, v, 2);
}

/// PUC 5.3+ `lua_seti`: `t[n] = value` (`t` at `idx`), the value on top,
/// through `__newindex`; pops the value.
///
/// # Safety
/// As [`luna_capi_lua_settable`].
// SAFETY: no other item in the link is named `luna_capi_lua_seti`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_seti(L: *mut LuaState, idx: c_int, n: i64) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = api.get_or_nil(idx);
    let v = api.get_or_nil(-1);
    set_and_pop(&mut api, t, Value::Int(n), v, 1);
}

/// PUC `lua_rawset`: the raw `t[key] = value` into the table at `idx`,
/// the value on top and the key below it; pops both. A nil or NaN key
/// raises, as does a read-only table (Redis).
///
/// # Safety
/// As [`luna_capi_lua_settable`]; the value at `idx` is a table.
// SAFETY: no other item in the link is named `luna_capi_lua_rawset`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_rawset(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let k = api.get_or_nil(-2);
    let v = api.get_or_nil(-1);
    raw_set_and_pop(&mut api, idx, k, v, 2);
}

/// PUC 5.3+ `lua_rawseti`: the raw `t[n] = value` into the table at
/// `idx`; pops the value.
///
/// # Safety
/// As [`luna_capi_lua_rawset`].
// SAFETY: no other item in the link is named `luna_capi_lua_rawseti`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_rawseti(L: *mut LuaState, idx: c_int, n: i64) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(-1);
    raw_set_and_pop(&mut api, idx, Value::Int(n), v, 1);
}

/// 5.1 and 5.2 `lua_rawseti`, whose key is an `int`.
///
/// # Safety
/// As [`luna_capi_lua_rawset`].
// SAFETY: no other item in the link is named `luna_capi_luna_rawseti_51`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_rawseti_51(L: *mut LuaState, idx: c_int, n: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(-1);
    raw_set_and_pop(&mut api, idx, Value::Int(i64::from(n)), v, 1);
}

/// PUC 5.2+ `lua_rawsetp`: the raw `t[p] = value` into the table at
/// `idx`, `p` a light userdata; pops the value.
///
/// # Safety
/// As [`luna_capi_lua_rawset`].
// SAFETY: no other item in the link is named `luna_capi_lua_rawsetp`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_rawsetp(L: *mut LuaState, idx: c_int, p: *const c_void) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(-1);
    raw_set_and_pop(&mut api, idx, Value::LightUserdata(p.cast()), v, 1);
}

c_exports! {
    lua_settable => luna_c_lua_settable,
    lua_setfield => luna_c_lua_setfield,
    lua_seti => luna_c_lua_seti,
    lua_rawset => luna_c_lua_rawset,
    lua_rawseti => luna_c_lua_rawseti,
    luna_rawseti_51 => luna_c_luna_rawseti_51,
    lua_rawsetp => luna_c_lua_rawsetp,
}
