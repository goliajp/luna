//! Tables: reading fields with and without metamethods, making tables,
//! `lua_next`, and the globals table `lua_getglobal` reads. Writing fields
//! is in `tables_set.rs`. The functions that may raise are reached through
//! their C wrappers (`csrc/shim_values.c`).
//!
//! A key or value an operation works on stays on the C stack until the
//! operation is over, so a collection a metamethod causes keeps it.

use super::*;
use luna_core::runtime::Table;

/// The table at `idx`; PUC checks that it is one only with
/// `LUA_USE_APICHECK`, and anything else is undefined there.
pub(super) fn table_at(api: &mut Api, idx: c_int) -> Gc<Table> {
    match api.get(idx) {
        Some(Value::Table(t)) => t,
        _ => panic!("table expected"),
    }
}

/// `t[k]` with metamethods into the top slot, where `k` already is (or
/// any value the caller pushed in its place); its type.
pub(super) fn index_into_top(api: &mut Api, t: Value, k: Value) -> c_int {
    match api.vm.host_index(t, k) {
        Ok(v) => {
            api.set(-1, v);
            type_tag(v)
        }
        Err(e) => {
            api.raise(e);
            LUA_TNIL
        }
    }
}

/// The table `lua_getglobal` and `lua_setglobal` use: 5.1's
/// `LUA_GLOBALSINDEX`, later the registry's `LUA_RIDX_GLOBALS` entry.
pub(super) fn globals_value(api: &mut Api) -> Value {
    if api.version() == LuaVersion::Lua51 {
        Value::Table(api.thread_globals())
    } else {
        api.vm.host_registry().get_int(2)
    }
}

/// PUC `lua_gettable`: replace the key on top with `t[key]` (`t` at
/// `idx`), through `__index`; returns its type (5.3+).
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_gettable`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_gettable(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = api.get_or_nil(idx);
    let k = api.get_or_nil(-1);
    index_into_top(&mut api, t, k)
}

/// PUC `lua_getfield`: push `t[k]` (`t` at `idx`), through `__index`;
/// returns its type (5.3+).
///
/// # Safety
/// As [`luna_capi_lua_gettable`]; `k` is a NUL-terminated string.
// SAFETY: no other item in the link is named `luna_capi_lua_getfield`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_getfield(
    L: *mut LuaState,
    idx: c_int,
    k: *const c_char,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = api.get_or_nil(idx);
    // SAFETY: `k` is NUL-terminated (# Safety)
    let key = api.str(unsafe { c_bytes(k) }.unwrap_or_default());
    api.push(key);
    index_into_top(&mut api, t, key)
}

/// PUC 5.3+ `lua_geti`: push `t[n]` (`t` at `idx`), through `__index`;
/// returns its type.
///
/// # Safety
/// As [`luna_capi_lua_gettable`].
// SAFETY: no other item in the link is named `luna_capi_lua_geti`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_geti(L: *mut LuaState, idx: c_int, n: i64) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = api.get_or_nil(idx);
    api.push(Value::Int(n));
    index_into_top(&mut api, t, Value::Int(n))
}

/// PUC `lua_rawget`: replace the key on top with the table's raw value for
/// it; returns its type (5.3+).
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into; the value at `idx`
/// is a table.
// SAFETY: no other item in the link is named `lua_rawget`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawget(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = table_at(&mut api, idx);
    let v = t.get(api.get_or_nil(-1));
    api.set(-1, v);
    type_tag(v)
}

fn rawgeti(api: &mut Api, idx: c_int, n: i64) -> c_int {
    let t = table_at(api, idx);
    let v = t.get_int(n);
    api.push(v);
    type_tag(v)
}

/// PUC 5.3+ `lua_rawgeti`: push the table's raw value for key `n`;
/// returns its type.
///
/// # Safety
/// As [`lua_rawget`].
// SAFETY: no other item in the link is named `lua_rawgeti`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawgeti(L: *mut LuaState, idx: c_int, n: i64) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    rawgeti(&mut api, idx, n)
}

/// 5.1 and 5.2 `lua_rawgeti`, whose key is an `int`.
///
/// # Safety
/// As [`lua_rawget`].
// SAFETY: no other item in the link is named `luna_rawgeti_51`: PUC's
// liblua has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_rawgeti_51(L: *mut LuaState, idx: c_int, n: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    rawgeti(&mut api, idx, i64::from(n));
}

/// PUC 5.2+ `lua_rawgetp`: push the table's raw value for the light
/// userdata `p`; returns its type (5.3+).
///
/// # Safety
/// As [`lua_rawget`].
// SAFETY: no other item in the link is named `lua_rawgetp`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawgetp(L: *mut LuaState, idx: c_int, p: *const c_void) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = table_at(&mut api, idx);
    let v = t.get(Value::LightUserdata(p.cast()));
    api.push(v);
    type_tag(v)
}

/// PUC `lua_createtable`: push a new table with room for `narr` array
/// items and `nrec` other entries.
///
/// # Safety
/// As [`lua_rawget`], without the table.
// SAFETY: no other item in the link is named `lua_createtable`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_createtable(L: *mut LuaState, narr: c_int, nrec: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let narr = usize::try_from(narr).unwrap_or(0);
    let nrec = usize::try_from(nrec).unwrap_or(0);
    let t = api.vm.host_new_table(narr, nrec);
    api.push(Value::Table(t));
    api.vm.host_check_gc();
}

/// PUC `lua_next`: pop a key and push the table's next key and value
/// (returns 1), or push nothing at the end (returns 0); a key that is not
/// in the table raises "invalid key to 'next'".
///
/// # Safety
/// As [`luna_capi_lua_gettable`]; the value at `idx` is a table.
// SAFETY: no other item in the link is named `luna_capi_lua_next`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_next(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let t = table_at(&mut api, idx);
    let k = api.get_or_nil(-1);
    match api.vm.host_next(t, k) {
        Ok(Some((k, v))) => {
            api.set(-1, k);
            api.push(v);
            1
        }
        Ok(None) => {
            api.pop();
            0
        }
        Err(e) => {
            api.raise(e);
            0
        }
    }
}

c_exports! {
    lua_gettable => luna_c_lua_gettable,
    lua_getfield => luna_c_lua_getfield,
    lua_geti => luna_c_lua_geti,
    lua_next => luna_c_lua_next,
}
