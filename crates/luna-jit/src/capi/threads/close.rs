//! Closing a thread: `lua_closethread` and `lua_resetthread`.

use super::*;

/// Close `api`'s thread (PUC `luaE_resetthread`): run its pending `__close`
/// handlers and clear its stack, leaving the error object there if it died
/// by one or a handler raised one. A thread closing itself (5.5's
/// `lua_closethread(L, L)`) ends the resume that runs it instead.
fn close(api: &mut Api) -> c_int {
    let co = api.thread();
    if api.vm.host_main_thread().ptr_eq(co) {
        // outside a C function the main thread's stack holds what the host
        // pushed: its to-be-closed slots are closed, newest first, each
        // getting the error the one before raised
        if !api.st().calls.is_empty() {
            return LUA_OK;
        }
        let before = api.vm.special_errors();
        let r = super::tbc::close_with(api, 0, None);
        api.truncate(0);
        return match r {
            Ok(()) => LUA_OK,
            Err(LuaError(e)) => {
                api.push(e);
                let kind = api.vm.error_status(e, before);
                super::ccall::status_code(api.version(), kind)
            }
        };
    }
    if api.vm.host_is_running(co) {
        let e = api.vm.host_close_running();
        api.raise(e);
        return LUA_OK;
    }
    match api.vm.host_close_thread(co) {
        Ok(None) => LUA_OK,
        Ok(Some(e)) | Err(LuaError(e)) => {
            api.push(e);
            LUA_ERRRUN
        }
    }
}

/// PUC `lua_closethread` (5.4.6+).
///
/// # Safety
/// As [`lua_resume`]; called by the C wrapper, which throws what this
/// raises.
// SAFETY: no other item in the link is named `luna_capi_lua_closethread`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_closethread(
    L: *mut LuaState,
    _from: *mut LuaState,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    close(&mut api)
}

/// PUC 5.4 `lua_resetthread`, `lua_closethread(L, NULL)`.
///
/// # Safety
/// As [`luna_capi_lua_closethread`].
// SAFETY: no other item in the link is named `luna_capi_lua_resetthread`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_resetthread(L: *mut LuaState) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    close(&mut api)
}

c_exports! {
    lua_closethread => luna_c_lua_closethread,
    lua_resetthread => luna_c_lua_resetthread,
}
