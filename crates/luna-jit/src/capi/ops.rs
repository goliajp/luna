//! Operations with metamethods: arithmetic, comparison, concatenation,
//! length. Each may call a metamethod, which may raise; they are reached
//! through their C wrappers (`csrc/shim_values.c`). The operands stay on
//! the stack until the operation is over.

use super::*;
use luna_core::vm::exec::host_c::{HOST_OP_BNOT, HOST_OP_UNM};

/// `lua_arith`'s `op` of the state's dialect as the 5.3+ `LUA_OP*`
/// number: 5.2 numbers `+ - * / % ^` and unary minus 0 to 6.
fn arith_op(v: LuaVersion, op: c_int) -> u8 {
    let op = if v == LuaVersion::Lua52 {
        match op {
            0..=2 => op,
            3 => 5,
            4 => 3,
            5 => 4,
            6 => c_int::from(HOST_OP_UNM),
            _ => panic!("invalid option"),
        }
    } else {
        op
    };
    match u8::try_from(op) {
        Ok(op) if op <= HOST_OP_BNOT => op,
        _ => panic!("invalid option"),
    }
}

/// PUC 5.2+ `lua_arith`: replace the two top values with the result of
/// operation `op` on them (the top value alone for the unary ones),
/// through metamethods.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_arith`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_arith(L: *mut LuaState, op: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let op = arith_op(api.version(), op);
    let unary = op == HOST_OP_UNM || op == HOST_OP_BNOT;
    let (l, r) = if unary {
        let v = api.get_or_nil(-1);
        (v, v)
    } else {
        (api.get_or_nil(-2), api.get_or_nil(-1))
    };
    match api.vm.host_arith(op, l, r) {
        Ok(v) => {
            if !unary {
                api.pop();
            }
            api.set(-1, v);
        }
        Err(e) => api.raise(e),
    }
}

/// `a op b`: 0 equal, 1 less than, 2 less or equal; false when either
/// index has no value.
fn compare(api: &mut Api, i1: c_int, i2: c_int, op: c_int) -> c_int {
    let (Some(a), Some(b)) = (api.get(i1), api.get(i2)) else {
        return 0;
    };
    let r = match op {
        0 => api.vm.host_equal(a, b),
        1 => api.vm.host_less(a, b, false),
        2 => api.vm.host_less(a, b, true),
        _ => panic!("invalid option"),
    };
    match r {
        Ok(b) => c_int::from(b),
        Err(e) => {
            api.raise(e);
            0
        }
    }
}

/// PUC 5.2+ `lua_compare`: whether the values at `i1` and `i2` are equal
/// (`LUA_OPEQ`), the first less (`LUA_OPLT`) or less or equal
/// (`LUA_OPLE`), through metamethods; 0 when an index has no value.
///
/// # Safety
/// As [`luna_capi_lua_arith`].
// SAFETY: no other item in the link is named `luna_capi_lua_compare`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_compare(
    L: *mut LuaState,
    i1: c_int,
    i2: c_int,
    op: c_int,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    compare(&mut api, i1, i2, op)
}

/// PUC 5.1 `lua_equal`: `lua_compare` with `LUA_OPEQ`.
///
/// # Safety
/// As [`luna_capi_lua_arith`].
// SAFETY: no other item in the link is named `luna_capi_lua_equal`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_equal(L: *mut LuaState, i1: c_int, i2: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    compare(&mut api, i1, i2, 0)
}

/// PUC 5.1 `lua_lessthan`: `lua_compare` with `LUA_OPLT`.
///
/// # Safety
/// As [`luna_capi_lua_arith`].
// SAFETY: no other item in the link is named `luna_capi_lua_lessthan`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_lessthan(L: *mut LuaState, i1: c_int, i2: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    compare(&mut api, i1, i2, 1)
}

/// PUC `lua_concat`'s Rust side: replace the top `n` values with their
/// concatenation, through `__concat`; with `n` 0 push the empty string,
/// with 1 leave the value as it is. C functions of the C API that
/// concatenate call it too (`csrc/shim.h`).
///
/// # Safety
/// As [`luna_capi_lua_arith`]; the frame holds at least `n` values.
// SAFETY: no other item in the link is named `luna_capi_concat`; the C
// wrapper of `lua_concat` and the C API's own C functions are its callers
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_concat(L: *mut LuaState, n: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    match n {
        0 => {
            let s = api.str(b"");
            api.push(s);
        }
        1 => {}
        _ => {
            let n = usize::try_from(n).expect("lua_concat of a negative count");
            let top = api.top();
            let vals = api.stack()[top - n..].to_vec();
            match api.vm.host_concat(&vals) {
                Ok(v) => {
                    api.truncate(top - n);
                    api.push(v);
                }
                Err(e) => {
                    api.raise(e);
                    return;
                }
            }
        }
    }
    api.vm.host_check_gc();
}

/// PUC 5.2+ `lua_len`: push the length of the value at `idx`, through
/// `__len`.
///
/// # Safety
/// As [`luna_capi_lua_arith`].
// SAFETY: no other item in the link is named `luna_capi_lua_len`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_len(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let v = api.get_or_nil(idx);
    match api.vm.host_len(v) {
        Ok(n) => api.push(n),
        Err(e) => api.raise(e),
    }
}

c_exports! {
    lua_arith => luna_c_lua_arith,
    lua_compare => luna_c_lua_compare,
    lua_equal => luna_c_lua_equal,
    lua_lessthan => luna_c_lua_lessthan,
    lua_concat => luna_c_lua_concat,
    lua_len => luna_c_lua_len,
}
