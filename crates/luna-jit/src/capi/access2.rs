//! More access functions: lengths, pointers, threads and raw equality.

use super::*;

/// The raw length of a value (PUC `lua_rawlen`): bytes of a string or a
/// userdata block, the border of a table, else 0.
fn raw_len(api: &Api, v: Value) -> u64 {
    match v {
        Value::Str(s) => s.len() as u64,
        Value::Table(t) => t.len() as u64,
        Value::Userdata(u) => api.vm.host_block(u).map_or(0, |b| b.size() as u64),
        _ => 0,
    }
}

/// PUC `lua_rawlen` (5.2+).
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_rawlen`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawlen(L: *mut LuaState, idx: c_int) -> u64 {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    raw_len(&api, v)
}

/// PUC 5.1 `lua_objlen`: `lua_rawlen`, and a number is converted to a
/// string in place and measured.
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_objlen`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_objlen(L: *mut LuaState, idx: c_int) -> usize {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    if let Some(Value::Int(_) | Value::Float(_)) = api.get(idx) {
        return access::tolstring(&mut api, idx).map_or(0, |(_, n)| n);
    }
    let v = api.get_or_nil(idx);
    raw_len(&api, v) as usize
}

/// PUC `lua_tocfunction`: the C function a value wraps, or null (also for
/// luna's own library functions, which are not C functions).
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_tocfunction`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tocfunction(L: *mut LuaState, idx: c_int) -> Option<LuaCFunction> {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let Some(Value::Native(nc)) = api.get(idx) else {
        return None;
    };
    if nc.builtin != luna_core::runtime::Builtin::CFunction {
        return None;
    }
    let Value::LightUserdata(p) = nc.upvals[0] else {
        return None;
    };
    // SAFETY: upvalue 0 of a C function's native was made from a
    // `lua_CFunction` by `lua_pushcclosure`
    Some(unsafe { std::mem::transmute::<*const (), LuaCFunction>(p) })
}

/// The pointer C sees for a userdata value.
fn userdata_ptr(api: &Api, v: Value) -> *mut c_void {
    match v {
        Value::Userdata(u) => api.vm.host_userdata_ptr(u).cast(),
        Value::LightUserdata(p) => p.cast_mut().cast(),
        _ => std::ptr::null_mut(),
    }
}

/// PUC `lua_touserdata`: a full userdata's block, a light userdata's
/// pointer, else null.
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_touserdata`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_touserdata(L: *mut LuaState, idx: c_int) -> *mut c_void {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    userdata_ptr(&api, v)
}

/// PUC `lua_tothread`: the `lua_State` of a thread value, else null.
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_tothread`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tothread(L: *mut LuaState, idx: c_int) -> *mut LuaState {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    match api.get(idx) {
        Some(Value::Coro(co)) => state_of(api.vm, co),
        _ => std::ptr::null_mut(),
    }
}

/// PUC `lua_topointer`: the address `tostring` prints for a value; strings
/// have one from 5.4 on.
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_topointer`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_topointer(L: *mut LuaState, idx: c_int) -> *const c_void {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    match api.get_or_nil(idx) {
        Value::Table(t) => t.as_ptr().cast_const().cast(),
        Value::Closure(c) => c.as_ptr().cast_const().cast(),
        Value::Native(n) => n.as_ptr().cast_const().cast(),
        Value::Coro(co) => co.as_ptr().cast_const().cast(),
        Value::Str(s) if api.version() >= LuaVersion::Lua54 => s.as_ptr().cast_const().cast(),
        v @ (Value::Userdata(_) | Value::LightUserdata(_)) => userdata_ptr(&api, v).cast_const(),
        _ => std::ptr::null(),
    }
}

/// PUC `lua_rawequal`: whether both indices hold values that are equal
/// without metamethods; 0 when either has no value.
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_rawequal`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rawequal(L: *mut LuaState, idx1: c_int, idx2: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    match (api.get(idx1), api.get(idx2)) {
        (Some(a), Some(b)) => c_int::from(a.raw_eq(b)),
        _ => 0,
    }
}

/// PUC `lua_stringtonumber` (5.3+): push the number the string `s`
/// denotes and return its length plus one, or return 0 and push nothing.
///
/// # Safety
/// As [`lua_rawlen`]; `s` is a NUL-terminated string.
// SAFETY: no other item in the link is named `lua_stringtonumber`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_stringtonumber(L: *mut LuaState, s: *const c_char) -> usize {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    // SAFETY: `s` is NUL-terminated (# Safety)
    let Some(b) = (unsafe { c_bytes(s) }) else {
        return 0;
    };
    match api.vm.host_str_to_number(b) {
        Some(v) => {
            api.push(v);
            b.len() + 1
        }
        None => 0,
    }
}

/// PUC 5.5 `lua_numbertocstring`: write the number at `idx` as text into
/// `buff` (`LUA_N2SBUFFSZ`, 64 bytes); the bytes written including the
/// NUL, or 0 when the value is not a number.
///
/// # Safety
/// As [`lua_rawlen`]; `buff` has room for 64 bytes.
// SAFETY: no other item in the link is named `lua_numbertocstring`: the
// host does not link PUC's liblua next to this crate, which defines each
// `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_numbertocstring(
    L: *mut LuaState,
    idx: c_int,
    buff: *mut c_char,
) -> u32 {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = match api.get(idx) {
        Some(v @ (Value::Int(_) | Value::Float(_))) => v,
        _ => return 0,
    };
    let text = api.vm.host_basic_text(v);
    let len = text.len().min(63);
    // SAFETY: `buff` has room for 64 bytes (# Safety), and `len < 64`
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buff.cast::<u8>(), len);
        *buff.add(len) = 0;
    }
    len as u32 + 1
}

/// `lua_type(L, idx) == LUA_TNIL`: the headers make `lua_isnil` the macro
/// PUC has; this is for callers that bind it by name.
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_isnil`: the host does not
// link PUC's liblua next to this crate, which defines each `lua_*` symbol
// once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isnil(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    c_int::from(unsafe { access::lua_type(L, idx) } == LUA_TNIL)
}

/// `lua_type(L, idx) == LUA_TBOOLEAN`, for callers that bind it by name.
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_isboolean`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isboolean(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    c_int::from(unsafe { access::lua_type(L, idx) } == LUA_TBOOLEAN)
}

/// `lua_type(L, idx) == LUA_TFUNCTION`, for callers that bind it by name.
///
/// # Safety
/// As [`lua_rawlen`].
// SAFETY: no other item in the link is named `lua_isfunction`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isfunction(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    c_int::from(unsafe { access::lua_type(L, idx) } == LUA_TFUNCTION)
}
