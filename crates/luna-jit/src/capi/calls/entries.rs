//! The C API's call, protected call and yield entries.

use super::*;

/// PUC `lua_callk`.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_callk`; its C wrapper calls it
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_callk(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    ctx: isize,
    k: Option<LuaKFunction>,
) {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    callk(&mut api, nargs, nresults, k.map(KFn::K53), ctx);
}

/// 5.2 `lua_callk`, whose context is an `int` and whose continuation is a
/// `lua_CFunction`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_luna_callk_52`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_callk_52(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    ctx: c_int,
    k: Option<LuaCFunction>,
) {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    callk(&mut api, nargs, nresults, k.map(KFn::K52), ctx as isize);
}

/// 5.1 `lua_call`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_lua_call`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_call(L: *mut LuaState, nargs: c_int, nresults: c_int) {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    callk(&mut api, nargs, nresults, None, 0);
}

/// PUC `lua_pcallk`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_lua_pcallk`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_pcallk(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    msgh: c_int,
    ctx: isize,
    k: Option<LuaKFunction>,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    pcallk(&mut api, nargs, nresults, msgh, k.map(KFn::K53), ctx)
}

/// 5.2 `lua_pcallk`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_luna_pcallk_52`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_pcallk_52(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    msgh: c_int,
    ctx: c_int,
    k: Option<LuaCFunction>,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    pcallk(
        &mut api,
        nargs,
        nresults,
        msgh,
        k.map(KFn::K52),
        ctx as isize,
    )
}

/// Call the function below the top `nargs` values in protected mode (PUC `lua_pcall`; a function in
/// 5.1, a macro over `lua_pcallk` after). It cannot yield, so it never throws and needs no C
/// wrapper.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pcall`: the host does not
// link PUC's liblua next to this crate, which defines each `lua_*` symbol
// once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pcall(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    msgh: c_int,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    pcallk(&mut api, nargs, nresults, msgh, None, 0)
}

/// PUC 5.1 `lua_cpcall`: call `func` with a light userdata of `ud` as its only argument, in
/// protected mode; the stack is left as it was, plus the error object on an error.
///
/// # Safety
/// As [`lua_pcall`]; `func` is a C function.
// SAFETY: no other item in the link is named `lua_cpcall`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_cpcall(
    L: *mut LuaState,
    func: LuaCFunction,
    ud: *mut c_void,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    let f = match super::push::new_c_closure(&mut api, func, 0) {
        Ok(f) => f,
        Err(e) => {
            api.push(e.0);
            return LUA_ERRMEM;
        }
    };
    api.push(f);
    api.push(Value::LightUserdata(ud.cast_const().cast()));
    pcallk(&mut api, 1, 0, 0, None, 0)
}

/// PUC `lua_yieldk`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_lua_yieldk`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_yieldk(
    L: *mut LuaState,
    nresults: c_int,
    ctx: isize,
    k: Option<LuaKFunction>,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    if yieldk(&mut api, nresults, k.map(KFn::K53), ctx) {
        api.g().raised = LUA_YIELD;
    }
    0
}

/// 5.2 `lua_yieldk`.
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_luna_yieldk_52`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_yieldk_52(
    L: *mut LuaState,
    nresults: c_int,
    ctx: c_int,
    k: Option<LuaCFunction>,
) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    if yieldk(&mut api, nresults, k.map(KFn::K52), ctx as isize) {
        api.g().raised = LUA_YIELD;
    }
    0
}

/// PUC 5.1 `lua_yield`: record the yield and return; the C function must
/// return the result (`return lua_yield(L, n);`).
///
/// # Safety
/// As [`luna_capi_lua_callk`].
// SAFETY: no other item in the link is named `luna_capi_lua_yield`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_yield(L: *mut LuaState, nresults: c_int) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    yieldk(&mut api, nresults, None, 0);
    -1
}

/// 5.2 `lua_getctx`: the status a continuation was called with
/// (`LUA_YIELD`, or the error of a protected call), and its context;
/// `LUA_OK` in the C function itself.
///
/// # Safety
/// `L` is a live thread of an open state; `ctx` is null or writable.
// SAFETY: no other item in the link is named `lua_getctx`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getctx(L: *mut LuaState, ctx: *mut c_int) -> c_int {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    let Some(c) = api.st().calls.last() else {
        return LUA_OK;
    };
    if c.k_status == LUA_OK {
        return LUA_OK;
    }
    let (status, k) = (c.k_status, c.k_ctx);
    if !ctx.is_null() {
        // SAFETY: `ctx` is non-null and writable (# Safety)
        unsafe { *ctx = k as c_int };
    }
    status
}

/// `lua_error`'s Rust side: the error object is on top of `L`.
///
/// # Safety
/// `L` is a live thread of an open state.
// SAFETY: no other item in the link is named `luna_capi_error_prepare`; the
// C function `lua_error` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_error_prepare(L: *mut LuaState) {
    // SAFETY: the caller's contract
    let mut api = unsafe { Api::new(L) };
    if api.top() == api.base() {
        api.push(Value::Nil);
    }
    api.g().err_from = L;
}
