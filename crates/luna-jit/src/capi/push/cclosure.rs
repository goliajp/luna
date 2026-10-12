//! Pushing C functions and C closures.

use super::*;

/// PUC `lua_pushcclosure`: push a C function with the top `n` values as
/// its upvalues (popped).
///
/// # Safety
/// As [`luna_capi_lua_pushlstring`]; `f` is a C function.
// SAFETY: no other item in the link is named `luna_capi_lua_pushcclosure`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_pushcclosure(L: *mut LuaState, f: LuaCFunction, n: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    match new_c_closure(&mut api, f, usize::try_from(n).unwrap_or(0)) {
        Ok(c) => api.push(c),
        Err(e) => api.raise(e),
    }
}

/// `lua_pushcclosure(L, f, 0)`: the headers make `lua_pushcfunction` the
/// macro PUC has; this is for callers that bind it by name.
///
/// # Safety
/// As [`luna_capi_lua_pushcclosure`].
// SAFETY: no other item in the link is named `luna_capi_lua_pushcfunction`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_pushcfunction(L: *mut LuaState, f: LuaCFunction) {
    // SAFETY: the caller's contract (# Safety)
    unsafe { luna_capi_lua_pushcclosure(L, f, 0) };
}

/// A C closure of `f` with `n` upvalues popped from the stack (PUC
/// `lua_pushcclosure`), not yet pushed. In 5.1 it takes the environment
/// of the running C function, or the thread's globals.
pub(in crate::capi) fn new_c_closure(
    api: &mut Api,
    f: LuaCFunction,
    n: usize,
) -> Result<Value, LuaError> {
    // the upvalues stay on the stack until the function holds them, so an
    // emergency collection while it is made keeps them
    let top = api.top();
    let from = top.saturating_sub(n).max(api.base());
    let env = if api.version() == LuaVersion::Lua51 {
        match api.running_c().map(|nc| nc.upvals[1]) {
            Some(Value::Table(t)) => Value::Table(t),
            _ => Value::Table(api.thread_globals()),
        }
    } else {
        Value::Nil
    };
    let mut upvals = Vec::with_capacity(api::C_UPVALS + n);
    upvals.push(Value::LightUserdata(f as *const ()));
    upvals.push(env);
    upvals.extend_from_slice(&api.stack()[from..top]);
    let (tramp, c) = (
        ccall::capi_trampoline as luna_core::runtime::value::NativeFn,
        luna_core::runtime::Builtin::CFunction,
    );
    // from 5.2 on a C function without upvalues is a light C function,
    // equal to every other push of the same pointer
    let c = if n == 0 && api.version() >= LuaVersion::Lua52 {
        api.vm
            .host_light_fn(f as usize, |vm| vm.builtin(tramp, &upvals, c))?
    } else {
        api.vm.builtin(tramp, &upvals, c)?
    };
    api.truncate(from);
    Ok(c)
}
