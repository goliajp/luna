//! The Rust half of the C API driven from Rust, in every dialect. The C
//! host programs in `tests/capi/` compare the whole API with PUC; these
//! check the Rust side directly: the functions that raise are called
//! through their Rust halves, which report an error instead of throwing.

use super::calls::*;
use super::debug::upvals::*;
use super::debug::*;
use super::gc::*;
use super::hooks::*;
use super::load::*;
use super::meta::*;
use super::ops::*;
use super::tables::*;
use super::tables_set::*;
use super::threads::*;
use super::userdata::*;
use super::*;
use std::ffi::CStr;

const DIALECTS: [c_int; 5] = [501, 502, 503, 504, 505];

/// The string at `idx`.
///
/// # Safety
/// `l` is a live state.
unsafe fn text(l: *mut LuaState, idx: c_int) -> String {
    // SAFETY: the caller's contract; a non-null result is NUL-terminated
    unsafe {
        let p = lua_tolstring(l, idx, std::ptr::null_mut());
        assert!(!p.is_null(), "a string at {idx}");
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// Whether the last Rust half raised; clears the report and drops the
/// error object it pushed.
///
/// # Safety
/// `l` is a live state.
unsafe fn take_raise(l: *mut LuaState) -> bool {
    // SAFETY: the caller's contract; the global record outlives the state
    unsafe {
        let g = (*l).g;
        let r = (*g).raised != 0;
        if r {
            (*g).raised = 0;
            (*g).err_from = std::ptr::null_mut();
            luna_capi_lua_settop(l, -2);
        }
        r
    }
}

/// Run `body` on a fresh state of each dialect.
fn each(body: impl Fn(*mut LuaState, c_int)) {
    for v in DIALECTS {
        let l = luna_newstate(v);
        assert!(!l.is_null());
        body(l, v);
        // SAFETY: `l` is the state made above, used by nothing else now
        unsafe { lua_close(l) };
    }
}

/// Load and run `src`, leaving its results.
///
/// # Safety
/// `l` is a live state with the libraries open.
unsafe fn run(l: *mut LuaState, src: &CStr) -> c_int {
    // SAFETY: the caller's contract
    unsafe {
        let st = luna_load_string(l, src);
        assert_eq!(st, LUA_OK, "{src:?} compiles");
        lua_pcall(l, 0, -1, 0)
    }
}

/// Compile `src` with `lua_load` over one buffer.
///
/// # Safety
/// `l` is a live state.
unsafe fn luna_load_string(l: *mut LuaState, src: &CStr) -> c_int {
    struct Once(Option<&'static [u8]>);
    unsafe extern "C" fn reader(
        _l: *mut LuaState,
        ud: *mut c_void,
        size: *mut usize,
    ) -> *const c_char {
        // SAFETY: `ud` is the `Once` below and `size` is writable
        unsafe {
            let once = &mut *ud.cast::<Once>();
            match once.0.take() {
                Some(b) => {
                    *size = b.len();
                    b.as_ptr().cast()
                }
                None => {
                    *size = 0;
                    std::ptr::null()
                }
            }
        }
    }
    let bytes: &'static [u8] = Box::leak(src.to_bytes().to_vec().into_boxed_slice());
    let mut once = Once(Some(bytes));
    let ud: *mut c_void = (&raw mut once).cast();
    // SAFETY: the caller's contract; `reader` matches `lua_Reader`
    unsafe {
        if (*(*l).g).version == 501 {
            luna_load_51(l, reader, ud, c"=unit".as_ptr())
        } else {
            lua_load(l, reader, ud, c"=unit".as_ptr(), std::ptr::null())
        }
    }
}

extern "C" fn c_double(l: *mut LuaState) -> c_int {
    // SAFETY: luna calls a C function with a live state
    unsafe {
        let n = lua_tointegerx(l, 1, std::ptr::null_mut());
        lua_pushinteger(l, n * 2);
    }
    1
}

#[test]
fn stack_push_and_access() {
    each(|l, _v| {
        // SAFETY: `l` is live for the whole closure
        unsafe {
            lua_pushinteger(l, 7);
            lua_pushnumber(l, 2.5);
            lua_pushstring(l, c"12".as_ptr());
            lua_pushboolean(l, 1);
            lua_pushnil(l);
            assert_eq!(lua_gettop(l), 5);
            assert_eq!(lua_absindex(l, -1), 5);
            assert_eq!(lua_tonumberx(l, 3, std::ptr::null_mut()), 12.0);
            assert_eq!(lua_isnumber(l, 3), 1);
            assert_eq!(lua_toboolean(l, 5), 0);
            assert_eq!(text(l, 2), "2.5");
            lua_rotate(l, 1, 1);
            assert_eq!(lua_type(l, 1), LUA_TNIL);
            lua_insert(l, 1);
            lua_remove(l, 1);
            lua_pushvalue(l, -1);
            lua_replace(l, 1);
            lua_copy(l, 2, 3);
            assert_eq!(lua_rawequal(l, 2, 3), 1);
            assert_eq!(lua_checkstack(l, 100), 1);
            luna_capi_lua_settop(l, 0);
            lua_pushlstring(l, b"a\0b".as_ptr().cast(), 3);
            let mut len = 0;
            lua_tolstring(l, -1, &mut len);
            assert_eq!(len, 3);
            assert_eq!(lua_stringtonumber(l, c"0x10".as_ptr()), 5);
            assert_eq!(lua_tointegerx(l, -1, std::ptr::null_mut()), 16);
            lua_pushcclosure(l, c_double, 0);
            assert_eq!(lua_iscfunction(l, -1), 1);
            assert!(lua_tocfunction(l, -1).is_some());
            assert_eq!(lua_pushthread(l), 1);
            assert_eq!(lua_tothread(l, -1), l);
            luna_capi_lua_settop(l, 0);
        }
    });
}

#[test]
fn tables_metatables_and_globals() {
    each(|l, _v| {
        // SAFETY: `l` is live for the whole closure
        unsafe {
            luaL_openlibs_for_test(l);
            lua_createtable(l, 2, 2);
            lua_pushinteger(l, 10);
            luna_capi_lua_setfield(l, 1, c"x".as_ptr());
            lua_pushstring(l, c"x".as_ptr());
            luna_capi_lua_gettable(l, 1);
            assert_eq!(lua_tointegerx(l, -1, std::ptr::null_mut()), 10);
            luna_capi_lua_settop(l, 1);
            lua_pushinteger(l, 5);
            luna_capi_lua_rawseti(l, 1, 1);
            lua_rawgeti(l, 1, 1);
            assert_eq!(lua_tointegerx(l, -1, std::ptr::null_mut()), 5);
            luna_capi_lua_settop(l, 1);
            lua_pushnil(l);
            let mut n = 0;
            while luna_capi_lua_next(l, 1) != 0 {
                n += 1;
                luna_capi_lua_settop(l, -2);
            }
            assert_eq!(n, 2);
            lua_pushnil(l);
            lua_pushinteger(l, 1);
            luna_capi_lua_rawset(l, 1);
            assert!(take_raise(l), "a nil key raises");
            luna_capi_lua_settop(l, 1);
            lua_createtable(l, 0, 0);
            luna_capi_lua_setmetatable(l, 1);
            assert_eq!(lua_getmetatable(l, 1), 1);
            luna_capi_lua_settop(l, 1);
            luna_capi_lua_setglobal(l, c"t".as_ptr());
            assert_eq!(luna_capi_lua_getglobal(l, c"t".as_ptr()), LUA_TTABLE);
            assert_eq!(lua_rawlen(l, -1), 1);
            luna_capi_lua_settop(l, 0);
        }
    });
}

/// Open the libraries through the Rust side the C `luaL_openlibs` uses.
///
/// # Safety
/// `l` is a live state.
unsafe fn luaL_openlibs_for_test(l: *mut LuaState) {
    // SAFETY: the caller's contract
    let api = unsafe { Api::new(l) };
    api.vm.open_all_libs();
}

#[test]
fn operators_and_userdata() {
    each(|l, v| {
        // SAFETY: `l` is live for the whole closure
        unsafe {
            luaL_openlibs_for_test(l);
            lua_pushinteger(l, 6);
            lua_pushinteger(l, 7);
            if v >= 502 {
                luna_capi_lua_arith(l, 2);
                assert_eq!(lua_tointegerx(l, -1, std::ptr::null_mut()), 42);
                lua_pushinteger(l, 1);
                assert_eq!(luna_capi_lua_compare(l, -1, -2, 1), 1);
            }
            lua_pushstring(l, c"a".as_ptr());
            lua_pushstring(l, c"b".as_ptr());
            luna_capi_concat(l, 2);
            assert_eq!(text(l, -1), "ab");
            luna_capi_lua_len(l, -1);
            assert_eq!(lua_tointegerx(l, -1, std::ptr::null_mut()), 2);
            luna_capi_lua_settop(l, 0);
            let p = lua_newuserdatauv(l, 16, 2);
            assert!(!p.is_null());
            assert_eq!(lua_touserdata(l, 1), p);
            assert_eq!(lua_rawlen(l, 1), 16);
            lua_pushinteger(l, 3);
            if v >= 504 {
                assert_eq!(lua_setiuservalue(l, 1, 1), 1);
                assert_eq!(lua_getiuservalue(l, 1, 1), LUA_TNUMBER);
            }
            luna_capi_lua_settop(l, 0);
        }
    });
}

#[test]
fn calls_errors_and_threads() {
    each(|l, v| {
        // SAFETY: `l` is live for the whole closure
        unsafe {
            luaL_openlibs_for_test(l);
            lua_pushcclosure(l, c_double, 0);
            luna_capi_lua_setglobal(l, c"double".as_ptr());
            assert_eq!(run(l, c"return double(21)"), LUA_OK);
            assert_eq!(lua_tointegerx(l, -1, std::ptr::null_mut()), 42);
            luna_capi_lua_settop(l, 0);
            assert_eq!(run(l, c"error('boom', 0)"), LUA_ERRRUN);
            assert_eq!(text(l, -1), "boom");
            luna_capi_lua_settop(l, 0);
            assert_eq!(
                run(
                    l,
                    c"return function(a) local b = coroutine.yield(a + 1) return b * 2 end"
                ),
                LUA_OK
            );
            let co = lua_newthread(l);
            lua_pushvalue(l, 1);
            lua_xmove(l, co, 1);
            lua_pushinteger(co, 1);
            let mut nres = 0;
            let st = match v {
                501 => luna_resume_51(co, 1),
                502 | 503 => luna_resume_52(co, l, 1),
                _ => lua_resume(co, l, 1, &mut nres),
            };
            assert_eq!(st, LUA_YIELD);
            assert_eq!(lua_status(co), LUA_YIELD);
            assert_eq!(lua_tointegerx(co, -1, std::ptr::null_mut()), 2);
            luna_capi_lua_settop(co, 0);
            lua_pushinteger(co, 5);
            let st = match v {
                501 => luna_resume_51(co, 1),
                502 | 503 => luna_resume_52(co, l, 1),
                _ => lua_resume(co, l, 1, &mut nres),
            };
            assert_eq!(st, LUA_OK);
            assert_eq!(lua_tointegerx(co, -1, std::ptr::null_mut()), 10);
            luna_capi_lua_settop(l, 0);
        }
    });
}

#[test]
fn debug_load_dump_and_gc() {
    unsafe extern "C" fn writer(
        _l: *mut LuaState,
        _p: *const c_void,
        sz: usize,
        ud: *mut c_void,
    ) -> c_int {
        // SAFETY: `ud` is the counter below
        unsafe { *ud.cast::<usize>() += sz };
        0
    }
    each(|l, v| {
        // SAFETY: `l` is live for the whole closure
        unsafe {
            luaL_openlibs_for_test(l);
            assert_eq!(luna_load_string(l, c"local a = 1 return a"), LUA_OK);
            let mut size = 0usize;
            let ud: *mut c_void = (&raw mut size).cast();
            let st = if v <= 502 {
                luna_capi_luna_dump_51(l, writer, ud)
            } else {
                luna_capi_lua_dump(l, writer, ud, 0)
            };
            assert_eq!(st, 0);
            assert!(size > 0);
            let mut up = std::ptr::null::<c_char>();
            if v >= 502 {
                up = lua_getupvalue(l, -1, 1);
            }
            assert!(v == 501 || !up.is_null());
            luna_capi_lua_settop(l, 0);
            let mut ar = [0u8; 512];
            assert_eq!(lua_getstack(l, 0, ar.as_mut_ptr().cast()), 0);
            assert_eq!(lua_gethookmask(l), 0);
            let count = if v >= 504 {
                luna_capi_gc(l, 3, 0, 0, 0)
            } else {
                luna_capi_luna_gc_51(l, 3, 0)
            };
            assert!(count > 0);
        }
    });
}
