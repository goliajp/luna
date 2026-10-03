//! State-wide functions: globals and the version.

use super::*;

/// PUC `lua_getglobal`: push the global `name` (through `_G`'s
/// `__index`) and return its type.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_getglobal`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_getglobal(L: *mut LuaState, name: *const c_char) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    // SAFETY: `name` is a NUL-terminated string (PUC's contract)
    let key = api.str(unsafe { c_bytes(name) }.unwrap_or_default());
    let g = Value::Table(api.thread_globals());
    api.push(key);
    match api.vm.index_with_mm(g, key) {
        Ok(v) => {
            api.pop();
            api.push(v);
            type_tag(v)
        }
        Err(e) => {
            api.pop();
            api.raise(e);
            LUA_TNIL
        }
    }
}

/// PUC `lua_setglobal`: pop the top value into the global `name`
/// (through `_G`'s `__newindex`).
///
/// # Safety
/// As [`luna_capi_lua_getglobal`].
// SAFETY: no other item in the link is named `luna_capi_lua_setglobal`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_setglobal(L: *mut LuaState, name: *const c_char) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    // SAFETY: `name` is a NUL-terminated string (PUC's contract)
    let key = api.str(unsafe { c_bytes(name) }.unwrap_or_default());
    let g = Value::Table(api.thread_globals());
    let v = api.get_or_nil(-1);
    api.push(key);
    let r = api.vm.set_index_with_mm(g, key, v);
    api.pop();
    api.pop();
    if let Err(e) = r {
        api.raise(e);
    }
}

/// luna's `lua_register(L, name, f)` export: install `f` as the global
/// `name`. The headers make `lua_register` the macro PUC has.
///
/// # Safety
/// As [`luna_capi_lua_getglobal`]; `f` is a C function.
// SAFETY: no other item in the link is named `luna_capi_lua_register`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_register(
    L: *mut LuaState,
    name: *const c_char,
    f: LuaCFunction,
) {
    // SAFETY: the caller's contract (# Safety)
    unsafe {
        push::lua_pushcclosure(L, f, 0);
        luna_capi_lua_setglobal(L, name);
    }
}

/// PUC 5.4+ `lua_version`: the version number of the state's dialect
/// (504.0 for 5.4); 5.2 and 5.3 return a pointer to it, which their
/// headers get from `luna_version_52`.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_version`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_version(L: *mut LuaState) -> f64 {
    // SAFETY: the caller's contract (# Safety)
    let api = unsafe { Api::new(L) };
    f64::from(api.vnum())
}

/// PUC 5.2 and 5.3 `lua_version`: the address of the version number of
/// the dialect; null `L` asks for the library's own, as in PUC.
///
/// # Safety
/// `L` is null or a live thread of an open state, and no other API call
/// on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `luna_version_52`: PUC's liblua
// has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_version_52(L: *mut LuaState) -> *const f64 {
    static VERSIONS: [f64; 2] = [502.0, 503.0];
    if L.is_null() {
        return &VERSIONS[1];
    }
    // SAFETY: the caller's contract (# Safety)
    let api = unsafe { Api::new(L) };
    if api.version() == LuaVersion::Lua52 {
        &VERSIONS[0]
    } else {
        &VERSIONS[1]
    }
}

/// Mark the table at `idx` read-only (`enabled` nonzero) or writable
/// again (Redis's `lua_enablereadonlytable`): see `Vm::set_readonly`. A
/// value that is not a table is left alone.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_enablereadonlytable`: the
// host does not link PUC's or Redis's liblua next to this crate, which
// defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_enablereadonlytable(L: *mut LuaState, idx: c_int, enabled: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    if let Some(Value::Table(t)) = api.get(idx) {
        api.vm.set_readonly(t, enabled != 0);
    }
}
