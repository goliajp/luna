//! Metatables, and the 5.1 environments of functions, threads and
//! userdata.

use super::*;
use luna_core::runtime::NativeClosure;

/// PUC `lua_getmetatable`: push the metatable of the value at `idx` (its
/// own, or its type's) and return 1, or push nothing and return 0.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_getmetatable`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getmetatable(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    match api.vm.host_metatable(v) {
        Some(mt) => {
            api.push(Value::Table(mt));
            1
        }
        None => 0,
    }
}

/// PUC `lua_setmetatable`: pop a table or nil and make it the metatable of
/// the value at `idx` (of its type, for a value with no metatable of its
/// own); returns 1. The object is marked for finalization as its version
/// does it; a read-only table raises.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises. The top value
/// is a table or nil.
// SAFETY: no other item in the link is named `luna_capi_lua_setmetatable`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_setmetatable(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let mt = match api.get_or_nil(-1) {
        Value::Nil => None,
        Value::Table(t) => Some(t),
        _ => panic!("table expected"),
    };
    let v = api.get_or_nil(idx);
    match api.vm.host_set_metatable(v, mt) {
        Ok(()) => {
            api.pop();
        }
        Err(e) => api.raise(e),
    }
    1
}

/// The C API's closure of a C function, which keeps its 5.1 environment
/// as native upvalue 1; `None` for luna's own library functions.
fn c_closure(nc: Gc<NativeClosure>) -> Option<Gc<NativeClosure>> {
    (nc.builtin == luna_core::runtime::Builtin::CFunction).then_some(nc)
}

/// PUC 5.1 `lua_getfenv`: push the environment of the function, thread or
/// userdata at `idx`, or nil for any other value. luna's library
/// functions have the globals as theirs.
///
/// # Safety
/// As [`lua_getmetatable`].
// SAFETY: no other item in the link is named `lua_getfenv`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getfenv(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let env = match api.get_or_nil(idx) {
        Value::Closure(cl) => api.vm.host_closure_env(cl),
        Value::Native(nc) => match c_closure(nc) {
            Some(nc) => nc.upvals[1],
            None => Value::Table(api.thread_globals()),
        },
        Value::Userdata(u) if api.vm.host_block(u).is_some() => {
            api.vm.host_uservalue(u, 1).unwrap_or(Value::Nil)
        }
        Value::Userdata(_) => Value::Table(api.thread_globals()),
        Value::Coro(co) => Value::Table(api.vm.host_thread_globals(co)),
        _ => Value::Nil,
    };
    api.push(env);
}

/// PUC 5.1 `lua_setfenv`: pop a table and make it the environment of the
/// function, thread or userdata at `idx`; returns 0 for any other value.
/// luna keeps a Lua function's environment in its `_ENV` cell, so one
/// without that cell returns 0 too, as `debug.setfenv` refuses it; a
/// library function's or library userdata's environment is accepted and
/// not kept.
///
/// # Safety
/// As [`lua_getmetatable`]; the top value is a table.
// SAFETY: no other item in the link is named `lua_setfenv`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setfenv(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let Value::Table(env) = api.get_or_nil(-1) else {
        panic!("table expected");
    };
    let done = match api.get_or_nil(idx) {
        Value::Closure(cl) => api.vm.host_set_closure_env(cl, env),
        Value::Native(nc) => {
            if let Some(nc) = c_closure(nc) {
                api.vm.host_set_native_upvalue(nc, 1, Value::Table(env));
            }
            true
        }
        Value::Userdata(u) => {
            if api.vm.host_block(u).is_some() {
                api.vm.host_set_uservalue(u, 1, Value::Table(env));
            }
            true
        }
        Value::Coro(co) => {
            api.vm.host_set_thread_globals(co, env);
            true
        }
        _ => false,
    };
    api.pop();
    c_int::from(done)
}

c_exports! {
    lua_setmetatable => luna_c_lua_setmetatable,
}
