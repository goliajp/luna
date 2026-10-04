//! Upvalues of functions on the stack: `lua_getupvalue`,
//! `lua_setupvalue`, `lua_upvalueid`, `lua_upvaluejoin`.

use super::super::api::C_UPVALS;
use super::super::*;
use super::c_api_fn;

/// Where upvalue `n` of a native lives in its upvalue array: a C
/// function's follow the C API's own.
fn native_upvalue(f: Value, n: c_int) -> Option<usize> {
    let Value::Native(nc) = f else { return None };
    let skip = if c_api_fn(f).is_some() { C_UPVALS } else { 0 };
    let k = usize::try_from(n).ok().filter(|&k| k >= 1)?;
    (skip + k - 1 < nc.upvals.len()).then_some(skip + k - 1)
}

/// PUC `lua_getupvalue`: push upvalue `n` of the function at `funcindex`
/// and return its name ("" for a C function's), or return NULL.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_getupvalue`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getupvalue(
    L: *mut LuaState,
    funcindex: c_int,
    n: c_int,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let f = api.get_or_nil(funcindex);
    let (name, v) = match f {
        Value::Native(nc) => match native_upvalue(f, n) {
            Some(k) => (Vec::new(), nc.upvals[k]),
            None => return std::ptr::null(),
        },
        _ => match api.vm.host_upvalue(f, i64::from(n)) {
            Some((name, v)) => (name.into_bytes(), v),
            None => return std::ptr::null(),
        },
    };
    api.push(v);
    super::c_name(&mut api, &name)
}

/// PUC `lua_setupvalue`: pop the top value into upvalue `n` of the
/// function at `funcindex` and return its name, or return NULL and leave
/// the value.
///
/// # Safety
/// As [`lua_getupvalue`].
// SAFETY: no other item in the link is named `lua_setupvalue`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setupvalue(
    L: *mut LuaState,
    funcindex: c_int,
    n: c_int,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let f = api.get_or_nil(funcindex);
    let v = api.get_or_nil(-1);
    let name = match f {
        Value::Native(nc) => match native_upvalue(f, n) {
            Some(k) => {
                // SAFETY: the function is on the stack, so alive; no other
                // reference into it is live, and the borrow covers one store
                unsafe { nc.as_mut() }.upvals[k] = v;
                api.vm.heap.barrier_back(nc);
                Vec::new()
            }
            None => return std::ptr::null(),
        },
        _ => match api.vm.host_set_upvalue(f, i64::from(n), v) {
            Some(name) => name.into_bytes(),
            None => return std::ptr::null(),
        },
    };
    api.pop();
    super::c_name(&mut api, &name)
}

/// PUC `lua_upvalueid` (5.2+): an address that identifies upvalue `n` of
/// the function at `funcindex`, shared by the closures that share it; NULL
/// for an index out of range.
///
/// # Safety
/// As [`lua_getupvalue`].
// SAFETY: no other item in the link is named `lua_upvalueid`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_upvalueid(
    L: *mut LuaState,
    funcindex: c_int,
    n: c_int,
) -> *mut c_void {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let f = api.get_or_nil(funcindex);
    let id = match f {
        Value::Native(nc) => native_upvalue(f, n).map(|k| (&nc.upvals[k] as *const Value).cast()),
        _ => api.vm.host_upvalue_id(f, i64::from(n)),
    };
    id.map_or(std::ptr::null_mut(), |p| p.cast_mut().cast())
}

/// PUC `lua_upvaluejoin` (5.2+): make upvalue `n1` of the Lua function at
/// `funcindex1` refer to upvalue `n2` of the one at `funcindex2`.
///
/// # Safety
/// As [`lua_getupvalue`]; both are Lua functions with those upvalues.
// SAFETY: no other item in the link is named `lua_upvaluejoin`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_upvaluejoin(
    L: *mut LuaState,
    funcindex1: c_int,
    n1: c_int,
    funcindex2: c_int,
    n2: c_int,
) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let f1 = api.get_or_nil(funcindex1);
    let f2 = api.get_or_nil(funcindex2);
    api.vm
        .host_upvalue_join(f1, i64::from(n1), f2, i64::from(n2));
}
