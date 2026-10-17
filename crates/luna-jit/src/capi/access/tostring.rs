//! Strings from stack values: `lua_tolstring` and its relatives.

use super::*;

/// `lua_tolstring`'s work: a number at `idx` is converted in place; a
/// memory error when the string cannot be made.
pub(in crate::capi) fn tolstring(
    api: &mut Api,
    idx: c_int,
) -> Result<Option<(*const c_char, usize)>, LuaError> {
    let s = match api.get(idx) {
        Some(Value::Str(s)) => s,
        Some(v @ (Value::Int(_) | Value::Float(_))) => {
            let text = api.vm.host_basic_text(v);
            let Value::Str(s) = api.vm.host_str(&text)? else {
                unreachable!("a string value");
            };
            api.set(idx, Value::Str(s));
            // every version checks after converting a number
            api.vm.host_check_gc()?;
            s
        }
        _ => return Ok(None),
    };
    Ok(Some((s.as_c_ptr(), s.len())))
}

/// Write the length of `r` to `len` (unless null) and return its pointer.
///
/// # Safety
/// `len` is null or writable.
unsafe fn out_len(r: Option<(*const c_char, usize)>, len: *mut usize) -> *const c_char {
    if !len.is_null() {
        // SAFETY: `len` is non-null and writable (# Safety)
        unsafe { *len = r.map_or(0, |(_, n)| n) };
    }
    r.map_or(std::ptr::null(), |(p, _)| p)
}

c_exports! {
    lua_tolstring => luna_c_lua_tolstring,
    lua_tostring => luna_c_lua_tostring,
}

/// PUC `lua_tolstring`: the string at `idx` (a number there becomes one),
/// valid while the value stays on the stack; null for other values.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// `len` is null or writable; called by the C wrapper, which throws what
/// this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_tolstring`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_tolstring(
    L: *mut LuaState,
    idx: c_int,
    len: *mut usize,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let r = match tolstring(&mut api, idx) {
        Ok(r) => r,
        Err(e) => {
            api.raise(e);
            None
        }
    };
    // SAFETY: `len` is null or writable (# Safety)
    unsafe { out_len(r, len) }
}

/// `lua_tolstring` for the C side's panic function, which runs outside any
/// error boundary: a string that cannot be made gives null.
///
/// # Safety
/// `L` is a live thread of an open state; `len` is null or writable.
// SAFETY: no other item in the link is named `luna_capi_tolstring`; the C
// side is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_tolstring(
    L: *mut LuaState,
    idx: c_int,
    len: *mut usize,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let r = tolstring(&mut api, idx).unwrap_or(None);
    // SAFETY: `len` is null or writable (# Safety)
    unsafe { out_len(r, len) }
}

/// `lua_tolstring(L, idx, NULL)`: the headers make `lua_tostring` the macro
/// PUC has; this is for callers that bind it by name.
///
/// # Safety
/// As [`luna_capi_lua_tolstring`].
// SAFETY: no other item in the link is named `luna_capi_lua_tostring`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_tostring(L: *mut LuaState, idx: c_int) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    unsafe { luna_capi_lua_tolstring(L, idx, std::ptr::null_mut()) }
}
