//! C ABI integration tests. Each test simulates a C caller — `unsafe extern "C"`
//! is the only API surface used so we exercise exactly the bytes a real
//! `lua.h` consumer would.
#![allow(non_snake_case)]
#![allow(clippy::approx_constant)] // 3.14 is a float test fixture in this file, not π.

use luna_jit::capi::*;
use std::ffi::CString;

/// Helper that builds a NUL-terminated buffer for `lua_pushstring` /
/// `luaL_loadstring`. Owned by the test so it outlives the C call.
fn cs(s: &str) -> CString {
    CString::new(s).expect("no internal NUL")
}

#[test]
fn capi_state_lifecycle() {
    unsafe {
        let l = luaL_newstate();
        assert!(!l.is_null());
        luaL_openlibs(l);
        lua_close(l);
    }
    // `lua_close(NULL)` is a no-op.
    unsafe { lua_close(std::ptr::null_mut()) };
}

#[test]
fn capi_load_and_pcall_basic() {
    unsafe {
        let l = luaL_newstate();
        luaL_openlibs(l);
        let src = cs("return 1 + 2");
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        // After load, the chunk is at the top.
        assert_eq!(lua_gettop(l), 1);
        assert_eq!(lua_type(l, -1), LUA_TFUNCTION);
        let status = lua_pcall(l, 0, 1, 0);
        assert_eq!(status, LUA_OK);
        assert_eq!(lua_gettop(l), 1);
        assert_eq!(lua_isinteger(l, -1), 1);
        assert_eq!(lua_tointeger(l, -1), 3);
        lua_close(l);
    }
}

#[test]
fn capi_pcall_args_and_multi_results() {
    unsafe {
        let l = luaL_newstate();
        luaL_openlibs(l);
        let src = cs("local a, b = ...; return a + b, a * b");
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        lua_pushinteger(l, 6);
        lua_pushinteger(l, 7);
        let status = lua_pcall(l, 2, 2, 0);
        assert_eq!(status, LUA_OK);
        assert_eq!(lua_gettop(l), 2);
        assert_eq!(lua_tointeger(l, -2), 13);
        assert_eq!(lua_tointeger(l, -1), 42);
        lua_close(l);
    }
}

#[test]
fn capi_pcall_runtime_error_pushes_message() {
    unsafe {
        let l = luaL_newstate();
        luaL_openlibs(l);
        let src = cs("error('boom', 0)");
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        let status = lua_pcall(l, 0, 0, 0);
        assert_eq!(status, LUA_ERRRUN);
        assert_eq!(lua_gettop(l), 1);
        assert_eq!(lua_type(l, -1), LUA_TSTRING);
        let cs_ptr = lua_tostring(l, -1);
        let msg = std::ffi::CStr::from_ptr(cs_ptr).to_str().unwrap();
        assert_eq!(msg, "boom");
        lua_close(l);
    }
}

#[test]
fn capi_load_syntax_error() {
    unsafe {
        let l = luaL_newstate();
        let src = cs("local 1bad = 2");
        let status = luaL_loadstring(l, src.as_ptr());
        assert_eq!(status, LUA_ERRSYNTAX);
        assert_eq!(lua_gettop(l), 1);
        assert_eq!(lua_type(l, -1), LUA_TSTRING);
        lua_close(l);
    }
}

#[test]
fn capi_globals_round_trip() {
    unsafe {
        let l = luaL_newstate();
        luaL_openlibs(l);
        // Host write → Lua read.
        lua_pushinteger(l, 99);
        let name = cs("from_host");
        lua_setglobal(l, name.as_ptr());
        assert_eq!(lua_gettop(l), 0);
        let src = cs("return from_host");
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        assert_eq!(lua_pcall(l, 0, 1, 0), LUA_OK);
        assert_eq!(lua_tointeger(l, -1), 99);
        lua_pop(l, 1);

        // Lua write → host read.
        let src = cs("from_lua = 'set by lua'");
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        assert_eq!(lua_pcall(l, 0, 0, 0), LUA_OK);
        let name = cs("from_lua");
        let ty = lua_getglobal(l, name.as_ptr());
        assert_eq!(ty, LUA_TSTRING);
        let cs_ptr = lua_tostring(l, -1);
        let s = std::ffi::CStr::from_ptr(cs_ptr).to_str().unwrap();
        assert_eq!(s, "set by lua");
        lua_close(l);
    }
}

#[test]
fn capi_stack_push_pop_settop() {
    unsafe {
        let l = luaL_newstate();
        lua_pushinteger(l, 1);
        lua_pushinteger(l, 2);
        lua_pushinteger(l, 3);
        assert_eq!(lua_gettop(l), 3);
        lua_pop(l, 1);
        assert_eq!(lua_gettop(l), 2);
        lua_settop(l, 0);
        assert_eq!(lua_gettop(l), 0);
        lua_settop(l, 4);
        assert_eq!(lua_gettop(l), 4);
        // Newly-grown slots are nil.
        assert_eq!(lua_isnil(l, -1), 1);
        lua_close(l);
    }
}

#[test]
fn capi_type_queries() {
    unsafe {
        let l = luaL_newstate();
        lua_pushnil(l);
        lua_pushboolean(l, 1);
        lua_pushinteger(l, 42);
        lua_pushnumber(l, 3.14);
        let s = cs("hello");
        lua_pushstring(l, s.as_ptr());
        // Expect (top → bottom): str, num, int, bool, nil.
        assert_eq!(lua_isstring(l, -1), 1);
        assert_eq!(lua_isnumber(l, -2), 1);
        assert_eq!(lua_isinteger(l, -3), 1);
        assert_eq!(lua_isboolean(l, -4), 1);
        assert_eq!(lua_isnil(l, -5), 1);
        assert_eq!(lua_type(l, -1), LUA_TSTRING);
        assert_eq!(lua_type(l, -2), LUA_TNUMBER);
        assert_eq!(lua_type(l, -3), LUA_TNUMBER);
        assert_eq!(lua_type(l, -4), LUA_TBOOLEAN);
        assert_eq!(lua_type(l, -5), LUA_TNIL);
        // Numeric coercions via lua_tostring should produce the
        // human-readable form.
        let cs_ptr = lua_tostring(l, -3);
        let int_str = std::ffi::CStr::from_ptr(cs_ptr).to_str().unwrap();
        assert_eq!(int_str, "42");
        lua_close(l);
    }
}

#[test]
fn capi_lua_version_returns_505() {
    unsafe {
        let l = luaL_newstate();
        assert_eq!(lua_version(l), 505);
        lua_close(l);
    }
}

/// A C-side callback that reads two integers and pushes their sum.
/// Exercised by `capi_register_c_callback`. Mirrors what a real C host
/// would write — only the public extern surface is used.
#[allow(non_snake_case)]
extern "C" fn c_add(L: *mut LuaState) -> std::os::raw::c_int {
    unsafe {
        let a = lua_tointeger(L, 1);
        let b = lua_tointeger(L, 2);
        lua_pushinteger(L, a + b);
        1 // one result
    }
}

/// A C callback that takes a string and returns its length as an integer.
#[allow(non_snake_case)]
extern "C" fn c_strlen(L: *mut LuaState) -> std::os::raw::c_int {
    unsafe {
        let s = lua_tostring(L, 1);
        if s.is_null() {
            lua_pushinteger(L, -1);
            return 1;
        }
        let len = std::ffi::CStr::from_ptr(s).to_bytes().len() as i64;
        lua_pushinteger(L, len);
        1
    }
}

#[test]
fn capi_register_c_callback() {
    unsafe {
        let l = luaL_newstate();
        luaL_openlibs(l);
        let name = cs("c_add");
        lua_register(l, name.as_ptr(), c_add);
        let src = cs("return c_add(40, 2)");
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        assert_eq!(lua_pcall(l, 0, 1, 0), LUA_OK);
        assert_eq!(lua_tointeger(l, -1), 42);
        lua_close(l);
    }
}

#[test]
fn capi_push_c_function_and_call_from_lua() {
    unsafe {
        let l = luaL_newstate();
        luaL_openlibs(l);
        // Push the C function and store it in a global.
        lua_pushcfunction(l, c_strlen);
        let name = cs("c_strlen");
        lua_setglobal(l, name.as_ptr());
        let src = cs("return c_strlen('hello, popen!')");
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        assert_eq!(lua_pcall(l, 0, 1, 0), LUA_OK);
        assert_eq!(lua_tointeger(l, -1), 13);
        lua_close(l);
    }
}

/// Callback that intentionally pushes nothing — exercise the
/// zero-result return path.
extern "C" fn c_void(_: *mut LuaState) -> std::os::raw::c_int {
    0
}

#[test]
fn capi_zero_result_callback() {
    unsafe {
        let l = luaL_newstate();
        luaL_openlibs(l);
        let name = cs("c_void");
        lua_register(l, name.as_ptr(), c_void);
        let src = cs("return select('#', c_void(1, 2, 3))");
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        assert_eq!(lua_pcall(l, 0, 1, 0), LUA_OK);
        assert_eq!(lua_tointeger(l, -1), 0);
        lua_close(l);
    }
}

#[test]
fn capi_pushvalue_duplicates() {
    unsafe {
        let l = luaL_newstate();
        lua_pushinteger(l, 7);
        lua_pushvalue(l, -1);
        assert_eq!(lua_gettop(l), 2);
        assert_eq!(lua_tointeger(l, -1), 7);
        assert_eq!(lua_tointeger(l, -2), 7);
        lua_close(l);
    }
}
