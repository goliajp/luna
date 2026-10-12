//! A string stored by `lua_setlocal` into a running numeric `for` loop's
//! limit raises `'for' state corrupted` at the loop's next step, integer and
//! float loops alike: the steps that do not test the limit's tag see the
//! change through the loop's step (luna-core `num_words`).

use luna_jit::capi::*;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

// SAFETY: the C API exports these under these names with these signatures
// (the first five in its C half). C sees `lua_State` as opaque
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn luaL_openlibs(L: *mut LuaState);
    fn luaL_loadstring(L: *mut LuaState, s: *const c_char) -> c_int;
    fn lua_pushstring(L: *mut LuaState, s: *const c_char) -> *const c_char;
    fn lua_pushcclosure(L: *mut LuaState, f: LuaCFunction, n: c_int);
    fn lua_tolstring(L: *mut LuaState, idx: c_int, len: *mut usize) -> *const c_char;
    fn lua_getstack(L: *mut LuaState, level: c_int, ar: *mut c_void) -> c_int;
    fn lua_setlocal(L: *mut LuaState, ar: *const c_void, n: c_int) -> *const c_char;
}

/// `set(n)`: local `n` of the calling Lua function := `"x"`.
extern "C" fn set(l: *mut LuaState) -> c_int {
    // room for any dialect's `lua_Debug`
    let mut ar = [0u64; 64];
    // SAFETY: `l` is the state calling this function; `ar` outlives the calls
    unsafe {
        let n = lua_tointegerx(l, 1, std::ptr::null_mut()) as c_int;
        assert_eq!(lua_getstack(l, 1, ar.as_mut_ptr().cast()), 1);
        lua_pushstring(l, c"x".as_ptr());
        assert!(!lua_setlocal(l, ar.as_ptr().cast(), n).is_null());
    }
    0
}

#[test]
fn a_limit_set_by_lua_setlocal_raises() {
    // the limit's local: 5.3 `(for limit)`, 5.4 the second `(for state)`
    // (an integer loop's count), 5.5 the first
    for (version, limit) in [(503, 2), (504, 2), (505, 1)] {
        for range in ["1, 3", "0.5, 3", "3.5, 0, -1"] {
            let src = CString::new(format!(
                "local set = ... local ok, e = pcall(function() for i = {range} do set({limit}) end end) \
                 return tostring(ok) .. ' ' .. tostring(e)"
            ))
            .unwrap();
            // SAFETY: `l` comes from `luna_newstate` and is used on this
            // thread only, until `lua_close`; `src` outlives the calls
            let got = unsafe {
                let l = luna_newstate(version);
                assert!(!l.is_null());
                luaL_openlibs(l);
                assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
                lua_pushcclosure(l, set, 0);
                assert_eq!(lua_pcall(l, 1, 1, 0), LUA_OK);
                let got = CStr::from_ptr(lua_tolstring(l, -1, std::ptr::null_mut()))
                    .to_string_lossy()
                    .into_owned();
                lua_close(l);
                got
            };
            assert!(
                got.starts_with("false ") && got.ends_with("'for' state corrupted"),
                "{version} {range}: {got}"
            );
        }
    }
}
