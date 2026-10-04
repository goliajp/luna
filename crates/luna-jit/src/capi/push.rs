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

/// Push the bytes `s[..len]` as a string and return the string's own
/// bytes (NUL-terminated); `s` may be null when `len` is 0.
fn push_bytes(api: &mut Api, bytes: &[u8]) -> *const c_char {
    let st = api.vm.heap.intern(bytes);
    api.push(Value::Str(st));
    st.as_c_ptr()
}

/// PUC `lua_pushlstring`: push `len` bytes from `s` (embedded zeros
/// included) and return the copy Lua keeps.
///
/// # Safety
/// As [`lua_pushnil`]; `s` points at `len` readable bytes, or is null with
/// `len` 0.
// SAFETY: no other item in the link is named `lua_pushlstring`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushlstring(
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

/// `lua_pushlstring` for the C side (`lua_pushfstring`).
///
/// # Safety
/// As [`lua_pushlstring`].
// SAFETY: no other item in the link is named `luna_capi_pushlstring`; the C
// side is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_pushlstring(
    L: *mut LuaState,
    s: *const c_char,
    len: usize,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    unsafe { lua_pushlstring(L, s, len) }
}

/// PUC `lua_pushstring`: push the NUL-terminated `s`, or nil for null;
/// returns the copy Lua keeps (null for null).
///
/// # Safety
/// As [`lua_pushnil`]; `s` is null or a NUL-terminated string.
// SAFETY: no other item in the link is named `lua_pushstring`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushstring(L: *mut LuaState, s: *const c_char) -> *const c_char {
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
/// back to `falloc` at once, instead of keeping it until the string dies.
///
/// # Safety
/// As [`lua_pushnil`]; `s` points at `len + 1` readable bytes; `falloc` is
/// null or a function that frees `s` when called as `falloc(ud, s, len +
/// 1, 0)`.
// SAFETY: no other item in the link is named `lua_pushexternalstring`: the
// host does not link PUC's liblua next to this crate, which defines each
// `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushexternalstring(
    L: *mut LuaState,
    s: *const c_char,
    len: usize,
    falloc: Option<state::LuaAlloc>,
    ud: *mut c_void,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let p = unsafe { lua_pushlstring(L, s, len) };
    if let Some(f) = falloc {
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
/// As [`lua_pushnil`]; `f` is a C function.
// SAFETY: no other item in the link is named `lua_pushcclosure`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushcclosure(L: *mut LuaState, f: LuaCFunction, n: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let c = calls::new_c_closure(&mut api, f, usize::try_from(n).unwrap_or(0));
    api.push(c);
}

/// `lua_pushcclosure(L, f, 0)`: the headers make `lua_pushcfunction` the
/// macro PUC has; this is for callers that bind it by name.
///
/// # Safety
/// As [`lua_pushcclosure`].
// SAFETY: no other item in the link is named `lua_pushcfunction`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushcfunction(L: *mut LuaState, f: LuaCFunction) {
    // SAFETY: the caller's contract (# Safety)
    unsafe { lua_pushcclosure(L, f, 0) };
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
