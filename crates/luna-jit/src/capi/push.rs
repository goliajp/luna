//! Push functions (C -> stack).

use super::*;

/// PUC `lua_pushnil`.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pushnil`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushnil(L: *mut LuaState) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    api.push(Value::Nil);
}

/// PUC `lua_pushnumber`.
///
/// # Safety
/// As [`lua_pushnil`].
// SAFETY: no other item in the link is named `lua_pushnumber`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushnumber(L: *mut LuaState, n: f64) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    api.push(Value::Float(n));
}

/// PUC `lua_pushinteger`. Up to 5.2, where numbers have no integer
/// subtype, an integer that a double holds exactly is pushed as a float.
///
/// # Safety
/// As [`lua_pushnil`].
// SAFETY: no other item in the link is named `lua_pushinteger`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushinteger(L: *mut LuaState, n: i64) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = int_value(api.version(), n);
    api.push(v);
}

/// An integer as the dialect stores it: ≤5.2 has only doubles, which luna
/// keeps as an integer when the double is integral, as its 5.1 and 5.2
/// arithmetic does.
pub(super) fn int_value(v: LuaVersion, n: i64) -> Value {
    if v <= LuaVersion::Lua52 && (n as f64) as i64 != n {
        Value::Float(n as f64)
    } else {
        Value::Int(n)
    }
}

c_exports! {
    lua_pushlstring => luna_c_lua_pushlstring,
    lua_pushstring => luna_c_lua_pushstring,
    lua_pushexternalstring => luna_c_lua_pushexternalstring,
    lua_pushcclosure => luna_c_lua_pushcclosure,
    lua_pushcfunction => luna_c_lua_pushcfunction,
}

/// Push `bytes` as a string and return the string's own bytes
/// (NUL-terminated), or raise the memory error and return null.
fn push_bytes(api: &mut Api, bytes: &[u8]) -> *const c_char {
    match api.vm.host_str(bytes) {
        Ok(v @ Value::Str(st)) => {
            api.push(v);
            st.as_c_ptr()
        }
        Ok(_) => unreachable!("a string value"),
        Err(e) => {
            api.raise(e);
            std::ptr::null()
        }
    }
}

/// PUC `lua_pushlstring`: push `len` bytes from `s` (embedded zeros
/// included) and return the copy Lua keeps.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// `s` points at `len` readable bytes, or is null with `len` 0; called by
/// the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_pushlstring`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_pushlstring(
    L: *mut LuaState,
    s: *const c_char,
    len: usize,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let bytes: &[u8] = if len == 0 {
        &[]
    } else {
        // SAFETY: `s` points at `len` readable bytes (# Safety)
        unsafe { std::slice::from_raw_parts(s.cast(), len) }
    };
    push_bytes(&mut api, bytes)
}

/// PUC `lua_pushstring`: push the NUL-terminated `s`, or nil for null;
/// returns the copy Lua keeps (null for null).
///
/// # Safety
/// As [`luna_capi_lua_pushlstring`]; `s` is null or a NUL-terminated
/// string.
// SAFETY: no other item in the link is named `luna_capi_lua_pushstring`;
// the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_pushstring(
    L: *mut LuaState,
    s: *const c_char,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    // SAFETY: `s` is null or NUL-terminated (# Safety)
    match unsafe { c_bytes(s) } {
        Some(b) => push_bytes(&mut api, b),
        None => {
            api.push(Value::Nil);
            std::ptr::null()
        }
    }
}

/// PUC 5.5 `lua_pushexternalstring`: push the `len` bytes at `s` (with a
/// NUL at `s[len]`) as a string. luna copies them and gives the buffer
/// back to `falloc` at once, instead of keeping it until the string dies;
/// when the copy cannot be made the buffer stays the caller's, as PUC
/// leaves it when it raises.
///
/// # Safety
/// As [`luna_capi_lua_pushlstring`]; `s` points at `len + 1` readable
/// bytes; `falloc` is null or a function that frees `s` when called as
/// `falloc(ud, s, len + 1, 0)`.
// SAFETY: no other item in the link is named
// `luna_capi_lua_pushexternalstring`; the C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_pushexternalstring(
    L: *mut LuaState,
    s: *const c_char,
    len: usize,
    falloc: Option<state::LuaAlloc>,
    ud: *mut c_void,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let p = unsafe { luna_capi_lua_pushlstring(L, s, len) };
    if let Some(f) = falloc
        && !p.is_null()
    {
        // SAFETY: `falloc` frees `s` this way (# Safety), and luna keeps no
        // pointer to it
        unsafe { f(ud, s.cast_mut().cast(), len + 1, 0) };
    }
    p
}

/// PUC `lua_pushboolean`.
///
/// # Safety
/// As [`lua_pushnil`].
// SAFETY: no other item in the link is named `lua_pushboolean`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushboolean(L: *mut LuaState, b: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    api.push(Value::Bool(b != 0));
}

/// PUC `lua_pushlightuserdata`.
///
/// # Safety
/// As [`lua_pushnil`].
// SAFETY: no other item in the link is named `lua_pushlightuserdata`: the
// host does not link PUC's liblua next to this crate, which defines each
// `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushlightuserdata(L: *mut LuaState, p: *mut c_void) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    api.push(Value::LightUserdata(p.cast_const().cast()));
}

/// PUC `lua_pushthread`: push `L`'s thread; 1 if it is the main thread.
///
/// # Safety
/// As [`lua_pushnil`].
// SAFETY: no other item in the link is named `lua_pushthread`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushthread(L: *mut LuaState) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let co = api.thread();
    api.push(Value::Coro(co));
    c_int::from(api.vm.host_main_thread().ptr_eq(co))
}

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

/// PUC 5.2 `lua_pushunsigned`: an unsigned 32-bit value as a number.
///
/// # Safety
/// As [`lua_pushnil`].
// SAFETY: no other item in the link is named `lua_pushunsigned`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushunsigned(L: *mut LuaState, n: u32) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    api.push(Value::Int(i64::from(n)));
}

/// The text of a number for `lua_pushfstring`'s `%d`, `%I` and `%f`: the
/// dialect's rendering, written to `buff` (at least 64 bytes); returns its
/// length.
///
/// # Safety
/// `L` is a live thread of an open state; `buff` has room for 64 bytes.
// SAFETY: no other item in the link is named `luna_capi_num2str`; the C
// side is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_num2str(
    L: *mut LuaState,
    isint: c_int,
    i: i64,
    n: f64,
    buff: *mut c_char,
) -> usize {
    // SAFETY: the caller's contract (# Safety)
    let api = unsafe { Api::new(L) };
    let v = if isint != 0 {
        int_value(api.version(), i)
    } else {
        Value::Float(n)
    };
    let text = api.vm.host_basic_text(v);
    let len = text.len().min(63);
    // SAFETY: `buff` has room for 64 bytes (# Safety), and `len < 64`
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buff.cast::<u8>(), len);
        *buff.add(len) = 0;
    }
    len
}

/// A C closure of `f` with `n` upvalues popped from the stack (PUC
/// `lua_pushcclosure`), not yet pushed. In 5.1 it takes the environment
/// of the running C function, or the thread's globals.
pub(super) fn new_c_closure(api: &mut Api, f: LuaCFunction, n: usize) -> Result<Value, LuaError> {
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
