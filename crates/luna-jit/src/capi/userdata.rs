//! Full userdata and their user values.

use super::*;
use luna_core::runtime::Userdata;

/// PUC's `SHRT_MAX` bound on a userdata's user values.
const MAX_UV: c_int = i16::MAX as c_int;

/// 5.1 `LUA_ENVIRONINDEX`.
const ENVIRONINDEX_51: c_int = -10001;

/// Push a new userdata of `size` zeroed bytes with `nuv` nil user values;
/// its block. In 5.1 the one user value is its environment: the running C
/// function's, or the thread's globals outside any.
fn new_userdata(api: &mut Api, size: usize, nuv: usize) -> *mut c_void {
    let u = api.vm.host_new_block(size, nuv);
    if api.version() == LuaVersion::Lua51 {
        let env = api.get_or_nil(ENVIRONINDEX_51);
        api.vm.host_set_uservalue(u, 1, env);
    }
    api.push(Value::Userdata(u));
    api.vm.host_check_gc();
    api.vm.host_userdata_ptr(u).cast()
}

/// PUC 5.4+ `lua_newuserdatauv`: push a new full userdata of `size`
/// zeroed bytes with `nuv` user values, all nil, and return its block.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into; `nuv` is in
/// `0..SHRT_MAX`.
// SAFETY: no other item in the link is named `lua_newuserdatauv`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_newuserdatauv(
    L: *mut LuaState,
    size: usize,
    nuv: c_int,
) -> *mut c_void {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let nuv = match usize::try_from(nuv) {
        Ok(n) if nuv < MAX_UV => n,
        _ => panic!("invalid value"),
    };
    new_userdata(&mut api, size, nuv)
}

/// PUC `lua_newuserdata` (a function up to 5.3, then a macro over
/// `lua_newuserdatauv` with one user value): push a new full userdata of
/// `size` zeroed bytes and return its block.
///
/// # Safety
/// As [`lua_newuserdatauv`].
// SAFETY: no other item in the link is named `lua_newuserdata`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_newuserdata(L: *mut LuaState, size: usize) -> *mut c_void {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    new_userdata(&mut api, size, 1)
}

/// The full userdata at `idx`; PUC checks that it is one only with
/// `LUA_USE_APICHECK`, and anything else is undefined there.
fn userdata_at(api: &mut Api, idx: c_int) -> Gc<Userdata> {
    match api.get(idx) {
        Some(Value::Userdata(u)) => u,
        _ => panic!("full userdata expected"),
    }
}

/// Push user value `n` of the userdata at `idx` and return its type, or
/// push nil and return `LUA_TNONE` when it has no such value.
fn get_uservalue(api: &mut Api, idx: c_int, n: c_int) -> c_int {
    let u = userdata_at(api, idx);
    let v = usize::try_from(n)
        .ok()
        .and_then(|n| api.vm.host_uservalue(u, n));
    api.push(v.unwrap_or(Value::Nil));
    v.map_or(LUA_TNONE, type_tag)
}

/// Pop the top value into user value `n` of the userdata at `idx`;
/// whether it has that value.
fn set_uservalue(api: &mut Api, idx: c_int, n: c_int) -> c_int {
    let v = api.get_or_nil(-1);
    let u = userdata_at(api, idx);
    let done = usize::try_from(n).is_ok_and(|n| api.vm.host_set_uservalue(u, n, v));
    api.pop();
    c_int::from(done)
}

/// PUC 5.4+ `lua_getiuservalue`: push user value `n` (from 1) of the full
/// userdata at `idx` and return its type; nil and `LUA_TNONE` when it has
/// no such value.
///
/// # Safety
/// As [`lua_newuserdatauv`]; the value at `idx` is a full userdata.
// SAFETY: no other item in the link is named `lua_getiuservalue`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getiuservalue(L: *mut LuaState, idx: c_int, n: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    get_uservalue(&mut api, idx, n)
}

/// PUC 5.4+ `lua_setiuservalue`: pop the top value into user value `n`
/// of the full userdata at `idx`; 0 when it has no such value.
///
/// # Safety
/// As [`lua_getiuservalue`].
// SAFETY: no other item in the link is named `lua_setiuservalue`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setiuservalue(L: *mut LuaState, idx: c_int, n: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    set_uservalue(&mut api, idx, n)
}

/// PUC 5.2 and 5.3 `lua_getuservalue` (later a macro over
/// `lua_getiuservalue`): push the user value of the full userdata at
/// `idx` (5.2: a table or nil) and return its type (5.3).
///
/// # Safety
/// As [`lua_getiuservalue`].
// SAFETY: no other item in the link is named `lua_getuservalue`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getuservalue(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    get_uservalue(&mut api, idx, 1)
}

/// PUC 5.2 and 5.3 `lua_setuservalue` (later a macro over
/// `lua_setiuservalue`): pop the top value into the user value of the
/// full userdata at `idx`. In 5.2 it is a table or nil.
///
/// # Safety
/// As [`lua_getiuservalue`]; in 5.2 the top value is a table or nil.
// SAFETY: no other item in the link is named `lua_setuservalue`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setuservalue(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    if api.version() == LuaVersion::Lua52
        && !matches!(api.get_or_nil(-1), Value::Nil | Value::Table(_))
    {
        panic!("table expected");
    }
    set_uservalue(&mut api, idx, 1)
}
