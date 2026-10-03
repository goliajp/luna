//! `lua_pcall`'s message handler, through the C API, in every dialect.
//! `capi_pcall_handler.txt` is what a C host built against PUC 5.1.5,
//! 5.2.4, 5.3.6, 5.4.9 and 5.5.1 printed for the same calls; this file
//! makes the same calls on luna and must print the same lines.
#![allow(non_snake_case)]

use luna_jit::capi::*;
use std::ffi::{CStr, CString};
use std::fmt::Write;
use std::os::raw::c_int;

const PUC: &str = include_str!("capi_pcall_handler.txt");

fn type_name(t: c_int) -> &'static str {
    match t {
        LUA_TNIL => "nil",
        LUA_TBOOLEAN => "boolean",
        LUA_TNUMBER => "number",
        LUA_TSTRING => "string",
        LUA_TTABLE => "table",
        LUA_TFUNCTION => "function",
        _ => "other",
    }
}

/// The string at `idx`, or `(null)`.
///
/// # Safety
/// `l` is an open state.
unsafe fn text(l: *mut LuaState, idx: c_int) -> String {
    // SAFETY: `l` is open (# Safety); a non-null result is read while its
    // value is still on the stack
    unsafe {
        let p = lua_tostring(l, idx);
        if p.is_null() {
            "(null)".into()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

/// # Safety
/// `l` is an open state.
unsafe fn show(l: *mut LuaState, out: &mut String, name: &str, st: c_int) {
    // SAFETY: `l` is open (# Safety) and -1 names the value lua_pcall pushed
    unsafe {
        let t = lua_type(l, -1);
        write!(
            out,
            "{name}: status={st} top={} type={}",
            lua_gettop(l),
            type_name(t)
        )
        .unwrap();
        if t == LUA_TSTRING || t == LUA_TNUMBER {
            write!(out, " value=[{}]", text(l, -1)).unwrap();
        }
        out.push('\n');
        lua_settop(l, 0);
    }
}

/// Push the function the chunk `src` returns.
///
/// # Safety
/// `l` is an open state.
unsafe fn push_fn(l: *mut LuaState, src: &str) {
    let src = CString::new(src).unwrap();
    // SAFETY: `l` is open (# Safety) and `src` is alive across the call
    unsafe {
        assert_eq!(luaL_loadstring(l, src.as_ptr()), LUA_OK);
        assert_eq!(lua_pcall(l, 0, 1, 0), LUA_OK);
    }
}

extern "C" fn c_handler(L: *mut LuaState) -> c_int {
    // SAFETY: `L` is the state luna passes to the C function it calls
    unsafe {
        let s = CString::new(format!("C:{}", text(L, 1))).unwrap();
        lua_pushstring(L, s.as_ptr());
        1
    }
}

/// A C function that makes its own `lua_pcall` with a handler and returns
/// the status and the error object.
extern "C" fn c_nested(L: *mut LuaState) -> c_int {
    // SAFETY: `L` is the state luna passes to the C function it calls
    unsafe {
        push_fn(L, "return function(m) return 'inner:' .. tostring(m) end");
        push_fn(L, "return function() error('deep', 0) end");
        let st = lua_pcall(L, 0, 0, -2);
        lua_pushinteger(L, st as i64);
        lua_pushvalue(L, -2);
        2
    }
}

extern "C" fn c_getbad(L: *mut LuaState) -> c_int {
    // SAFETY: `L` is the state luna passes to the C function it calls
    unsafe {
        lua_getglobal(L, c"bad".as_ptr());
        lua_pushstring(L, c"after".as_ptr());
    }
    1
}

extern "C" fn c_setbad(L: *mut LuaState) -> c_int {
    // SAFETY: `L` is the state luna passes to the C function it calls
    unsafe {
        lua_pushstring(L, c"v".as_ptr());
        lua_setglobal(L, c"bad".as_ptr());
        lua_pushstring(L, c"after".as_ptr());
    }
    1
}

/// Globals read and written through `_G`'s metamethods, and their errors
/// raised out of the C function that made the call.
///
/// # Safety
/// `l` is an open state.
unsafe fn globals(l: *mut LuaState, o: &mut String) {
    let mt = CString::new(
        "setmetatable(_G, {__index = function(t, k) if k == 'bad' then error('no global ' .. k) \
         end return 'idx:' .. k end, __newindex = function(t, k, v) if k == 'bad' then \
         error('cannot set ' .. k, 0) end rawset(t, k, v .. '!') end})",
    )
    .unwrap();
    // SAFETY: `l` is open (# Safety); every index names a slot pushed here
    unsafe {
        assert_eq!(luaL_loadstring(l, mt.as_ptr()), LUA_OK);
        assert_eq!(lua_pcall(l, 0, 0, 0), LUA_OK);
        lua_getglobal(l, c"missing".as_ptr());
        show(l, o, "getglobal_mm", 0);
        lua_pushstring(l, c"v".as_ptr());
        lua_setglobal(l, c"newg".as_ptr());
        lua_getglobal(l, c"newg".as_ptr());
        show(l, o, "setglobal_mm", 0);
        lua_pushcfunction(l, c_getbad);
        let st = lua_pcall(l, 0, 1, 0);
        show(l, o, "getglobal_error", st);
        lua_pushcfunction(l, c_setbad);
        let st = lua_pcall(l, 0, 1, 0);
        show(l, o, "setglobal_error", st);
        push_fn(l, "return function(m) return 'H:' .. m end");
        lua_pushcfunction(l, c_setbad);
        let st = lua_pcall(l, 0, 1, 1);
        show(l, o, "setglobal_error_handler", st);
    }
}

/// # Safety
/// `l` is an open state.
unsafe fn run(l: *mut LuaState, out: &mut String, name: &str, h: &str, f: &str, nargs: i64) {
    // SAFETY: `l` is open (# Safety); the handler sits at index 1
    unsafe {
        push_fn(l, h);
        push_fn(l, f);
        for i in 0..nargs {
            lua_pushinteger(l, i + 1);
        }
        let st = lua_pcall(l, nargs as c_int, 1, 1);
        show(l, out, name, st);
    }
}

const BOOM: &str = "return function() error('boom', 0) end";

/// The calls `capi_pcall_handler.txt` was made with, in the same order.
fn luna_output(version: c_int) -> String {
    let mut o = String::new();
    // SAFETY: `l` comes from `luna_newstate` and is used on this thread
    // only, until `lua_close`; every index names a slot pushed here
    unsafe {
        let l = luna_newstate(version);
        assert!(!l.is_null());
        luaL_openlibs(l);
        let errerr = if matches!(version, 502 | 503) {
            6
        } else {
            LUA_ERRERR
        };
        writeln!(o, "LUA_ERRRUN={LUA_ERRRUN} LUA_ERRERR={errerr}").unwrap();
        run(
            l,
            &mut o,
            "returns",
            "return function(m) return 'H:' .. m end",
            BOOM,
            0,
        );
        run(
            l,
            &mut o,
            "returns_table",
            "return function(m) return {m} end",
            BOOM,
            0,
        );
        run(
            l,
            &mut o,
            "returns_nothing",
            "return function(m) end",
            BOOM,
            0,
        );
        run(
            l,
            &mut o,
            "returns_two",
            "return function(m) return 'a', 'b' end",
            BOOM,
            0,
        );
        let typ = "return function(m) return type(m) end";
        run(
            l,
            &mut o,
            "nonstring_err",
            typ,
            "return function() error({}) end",
            0,
        );
        run(
            l,
            &mut o,
            "nil_err",
            typ,
            "return function() error() end",
            0,
        );
        run(
            l,
            &mut o,
            "runtime_err",
            "return function(m) return 'H:' .. m end",
            "return function() local t = nil; return t.x end",
            0,
        );
        run(
            l,
            &mut o,
            "args",
            "return function(m) return 'H:' .. m end",
            "return function(a, b) error(a + b, 0) end",
            2,
        );
        run(
            l,
            &mut o,
            "always_errors",
            "return function(m) error('again', 0) end",
            BOOM,
            0,
        );
        run(
            l,
            &mut o,
            "errors_once",
            "local n = 0; return function(m) n = n + 1; if n == 1 then error('again', 0) end; \
             return 'H' .. n .. ':' .. m end",
            BOOM,
            0,
        );
        run(
            l,
            &mut o,
            "runtime_err_in_handler",
            "return function(m) local t = nil; return t.x end",
            BOOM,
            0,
        );
        run(
            l,
            &mut o,
            "traceback",
            "return debug.traceback",
            "return function() error('boom') end",
            0,
        );
        run(
            l,
            &mut o,
            "no_error",
            "return function(m) return 'H:' .. m end",
            "return function() return 7 end",
            0,
        );
        lua_pushinteger(l, 42);
        push_fn(l, BOOM);
        let st = lua_pcall(l, 0, 1, 1);
        show(l, &mut o, "handler_number", st);
        lua_pushcfunction(l, c_handler);
        push_fn(l, BOOM);
        let st = lua_pcall(l, 0, 1, 1);
        show(l, &mut o, "c_handler", st);
        push_fn(l, "return function(m) return 'neg:' .. m end");
        push_fn(l, "return function(a) error('x' .. a, 0) end");
        lua_pushinteger(l, 5);
        let st = lua_pcall(l, 1, 1, -3);
        show(l, &mut o, "negative_index", st);
        push_fn(l, "return function(m) return 'deep:' .. m end");
        lua_pushinteger(l, 99);
        push_fn(l, "return function() error('y', 0) end");
        let st = lua_pcall(l, 0, 1, 1);
        writeln!(
            o,
            "handler_below: status={st} top={} [{}] [{}]",
            lua_gettop(l),
            text(l, 2),
            text(l, 3)
        )
        .unwrap();
        lua_settop(l, 0);
        push_fn(l, BOOM);
        let st = lua_pcall(l, 0, 1, 0);
        show(l, &mut o, "no_handler", st);
        push_fn(l, "return function() error() end");
        let st = lua_pcall(l, 0, 1, 0);
        show(l, &mut o, "no_handler_nil", st);
        lua_pushcfunction(l, c_nested);
        let st = lua_pcall(l, 0, 2, 0);
        writeln!(
            o,
            "nested: status={st} inner={} [{}]",
            text(l, 1),
            text(l, 2)
        )
        .unwrap();
        lua_settop(l, 0);
        run(
            l,
            &mut o,
            "inner_errerr_then_plain",
            "return function(m) return 'H:' .. tostring(m) .. '|' .. tostring(inner) end",
            "return function() local _; _, inner = xpcall(function() error('a') end, \
             function() error('b') end); error('plain', 0) end",
            0,
        );
        run(
            l,
            &mut o,
            "returns_errerr_text",
            "return function(m) return 'error in error handling' end",
            BOOM,
            0,
        );
        globals(l, &mut o);
        lua_close(l);
    }
    o
}

/// PUC's lines for one dialect, without the case luna's C API cannot make:
/// `c_raiser` needs `lua_error`, which it does not export.
fn puc_output(dialect: &str) -> String {
    // a Windows checkout may give the recording CRLF line endings
    let puc = PUC.replace("\r\n", "\n");
    let start = puc.find(&format!("== {dialect}\n")).unwrap() + dialect.len() + 4;
    let rest = &puc[start..];
    let section = &rest[..rest.find("\n== ").map_or(rest.len(), |i| i + 1)];
    section
        .lines()
        .filter(|l| !l.starts_with("c_raiser:"))
        .map(|l| format!("{l}\n"))
        .collect()
}

#[test]
fn pcall_message_handler_matches_puc() {
    let mut failed = Vec::new();
    for (dialect, version) in [
        ("5.1", 501),
        ("5.2", 502),
        ("5.3", 503),
        ("5.4", 504),
        ("5.5", 505),
    ] {
        let got = luna_output(version);
        let want = puc_output(dialect);
        if got != want {
            failed.push(format!("== {dialect}\n--- luna\n{got}--- puc\n{want}"));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Redis marks `_G` read-only; a C function writing a global then fails
/// as a script's assignment does, inside the `lua_pcall` that called it.
#[test]
fn setglobal_on_a_readonly_globals_table_raises() {
    for version in [501, 502, 503, 504, 505] {
        // SAFETY: `l` comes from `luna_newstate` and is used on this thread
        // only, until `lua_close`; every index names a slot pushed here
        unsafe {
            let l = luna_newstate(version);
            luaL_openlibs(l);
            lua_getglobal(l, c"_G".as_ptr());
            lua_enablereadonlytable(l, -1, 1);
            lua_settop(l, 0);
            lua_pushcfunction(l, c_setbad);
            let st = lua_pcall(l, 0, 1, 0);
            assert_eq!(st, LUA_ERRRUN, "{version}");
            assert_eq!(lua_gettop(l), 1);
            assert_eq!(
                text(l, -1),
                "Attempt to modify a readonly table",
                "{version}"
            );
            lua_settop(l, 0);
            // a script's assignment carries its position
            push_fn(l, "return function() bad = 1 end");
            let st = lua_pcall(l, 0, 0, 0);
            assert_eq!(st, LUA_ERRRUN);
            assert!(
                text(l, -1).ends_with(":1: Attempt to modify a readonly table"),
                "{}",
                text(l, -1)
            );
            lua_settop(l, 0);
            lua_getglobal(l, c"_G".as_ptr());
            lua_enablereadonlytable(l, -1, 0);
            lua_settop(l, 0);
            lua_pushcfunction(l, c_setbad);
            assert_eq!(lua_pcall(l, 0, 1, 0), LUA_OK);
            lua_getglobal(l, c"bad".as_ptr());
            assert_eq!(text(l, -1), "v");
            lua_close(l);
        }
    }
}

/// A C function sees its own arguments at index 1 on, whatever the host
/// left below the protected call (here the message handler).
#[test]
fn c_function_frame_starts_at_its_arguments() {
    extern "C" fn count(L: *mut LuaState) -> c_int {
        // SAFETY: `L` is the state luna passes to the C function it calls
        unsafe {
            let n = lua_gettop(L);
            let first = lua_tointeger(L, 1);
            lua_pushinteger(L, n as i64 * 100 + first);
        }
        1
    }
    // SAFETY: `l` comes from `luna_newstate` and is used on this thread
    // only, until `lua_close`; every index names a slot pushed here
    unsafe {
        let l = luna_newstate(505);
        luaL_openlibs(l);
        lua_pushinteger(l, 7);
        lua_pushinteger(l, 8);
        lua_pushcfunction(l, count);
        lua_pushinteger(l, 3);
        lua_pushinteger(l, 4);
        assert_eq!(lua_pcall(l, 2, 1, 0), LUA_OK);
        assert_eq!(lua_gettop(l), 3);
        assert_eq!(lua_tointeger(l, -1), 203);
        assert_eq!(lua_tointeger(l, 1), 7);
        lua_close(l);
    }
}

#[test]
fn newstate_rejects_unknown_versions() {
    assert!(luna_newstate(500).is_null());
    assert!(luna_newstate(506).is_null());
    let l = luna_newstate(501);
    // SAFETY: `l` is a state `luna_newstate` just made, closed once
    unsafe {
        assert_eq!(lua_version(l), 501);
        lua_close(l);
    }
}
