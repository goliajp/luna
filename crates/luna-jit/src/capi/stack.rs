//! Basic stack manipulation: top, indices, moving values around.

use super::*;

/// PUC `LUAI_MAXSTACK` (5.1: `LUAI_MAXCSTACK`, the room a C function may
/// ask for).
fn max_stack(v: LuaVersion) -> usize {
    if v == LuaVersion::Lua51 {
        8000
    } else {
        1_000_000
    }
}

/// PUC `lua_absindex` (5.2+): a negative stack index made positive;
/// pseudo-indices are returned as they are.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_absindex`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_absindex(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    if idx > 0 || idx <= api::registry_index(api.version()) {
        idx
    } else {
        api.gettop() + 1 + idx
    }
}

/// PUC `lua_gettop`: the number of values in the running function's frame.
///
/// # Safety
/// As [`lua_absindex`].
// SAFETY: no other item in the link is named `lua_gettop`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gettop(L: *mut LuaState) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    api.gettop()
}

/// `lua_settop`'s work: from 5.4 on, the to-be-closed values it drops are
/// closed, which may raise (its C wrapper throws that).
pub(super) fn settop(api: &mut Api, idx: c_int) {
    let base = api.base();
    let top = api.top();
    let new_top = if idx >= 0 {
        base + idx as usize
    } else {
        let d = (-idx - 1) as usize;
        top.saturating_sub(d).max(base)
    };
    if new_top < top {
        tbc::close_down_to(api, new_top);
        if api.raised() {
            return;
        }
        api.truncate(new_top);
    } else {
        let n = new_top - top;
        api.push_all(&vec![Value::Nil; n]);
    }
}

/// PUC `lua_settop`.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_settop`; the C
// wrapper `luna_c_lua_settop` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_settop(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    settop(&mut api, idx);
}

/// luna's `lua_pop` export: `lua_settop(L, -n - 1)`. The headers make
/// `lua_pop` the macro PUC has; this is for callers that bind it by name.
///
/// # Safety
/// As [`luna_capi_lua_settop`].
// SAFETY: no other item in the link is named `luna_capi_lua_pop`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_pop(L: *mut LuaState, n: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    settop(&mut api, -n - 1);
}

/// PUC `lua_pushvalue`: push a copy of the value at `idx`.
///
/// # Safety
/// As [`lua_absindex`].
// SAFETY: no other item in the link is named `lua_pushvalue`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushvalue(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    api.push(v);
}

/// Rotate the values from stack index `idx` to the top by `n` places
/// toward the top (negative: toward `idx`).
fn rotate(api: &mut Api, idx: c_int, n: c_int) {
    let Some(start) = api.abs_stack(idx) else {
        return;
    };
    let seg = &mut api.stack_mut()[start..];
    let len = seg.len();
    if len == 0 {
        return;
    }
    let k = (n.rem_euclid(len as c_int)) as usize;
    seg.rotate_right(k);
}

/// PUC `lua_rotate` (5.3+).
///
/// # Safety
/// As [`lua_absindex`].
// SAFETY: no other item in the link is named `lua_rotate`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_rotate(L: *mut LuaState, idx: c_int, n: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    rotate(&mut api, idx, n);
}

/// PUC `lua_copy` (5.2+): copy the value at `fromidx` to `toidx`, which
/// may be an upvalue pseudo-index.
///
/// # Safety
/// As [`lua_absindex`].
// SAFETY: no other item in the link is named `lua_copy`: the host does not
// link PUC's liblua next to this crate, which defines each `lua_*` symbol
// once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_copy(L: *mut LuaState, fromidx: c_int, toidx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(fromidx);
    api.set(toidx, v);
}

/// PUC `lua_insert` (a function up to 5.2, then a macro over
/// `lua_rotate`): move the top value to `idx`.
///
/// # Safety
/// As [`lua_absindex`].
// SAFETY: no other item in the link is named `lua_insert`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_insert(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    rotate(&mut api, idx, 1);
}

/// PUC `lua_remove` (a function up to 5.2): remove the value at `idx`,
/// shifting the ones above down.
///
/// # Safety
/// As [`lua_absindex`].
// SAFETY: no other item in the link is named `lua_remove`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_remove(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    if let Some(i) = api.abs_stack(idx) {
        api.stack_mut().remove(i);
    }
}

/// PUC `lua_replace` (a function up to 5.2): pop the top value into
/// `idx`, which may be a pseudo-index.
///
/// # Safety
/// As [`lua_absindex`].
// SAFETY: no other item in the link is named `lua_replace`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_replace(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(-1);
    api.set(idx, v);
    api.pop();
}

/// PUC `lua_checkstack`: whether the frame may grow by `n` more values.
/// The C stack grows on demand; only PUC's limit on its size refuses.
///
/// # Safety
/// As [`lua_absindex`].
// SAFETY: no other item in the link is named `lua_checkstack`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_checkstack(L: *mut LuaState, n: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let max = max_stack(api.version());
    let n = usize::try_from(n).unwrap_or(0);
    let used = if api.version() == LuaVersion::Lua51 {
        api.gettop() as usize
    } else {
        api.top()
    };
    c_int::from(n <= max && used + n <= max)
}

/// PUC `lua_xmove`: pop `n` values from `from` and push them on `to`, two
/// threads of the same state.
///
/// # Safety
/// `from` and `to` are live threads of the same open state, and no other
/// API call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_xmove`: the host does not
// link PUC's liblua next to this crate, which defines each `lua_*` symbol
// once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_xmove(from: *mut LuaState, to: *mut LuaState, n: c_int) {
    if from == to {
        return;
    }
    let vals = {
        // SAFETY: the caller's contract (# Safety)
        let mut api = unsafe { Api::new(from) };
        api.pop_n(usize::try_from(n).unwrap_or(0))
    };
    // SAFETY: the caller's contract; the context for `from` is gone
    let mut api = unsafe { Api::new(to) };
    api.push_all(&vals);
}
