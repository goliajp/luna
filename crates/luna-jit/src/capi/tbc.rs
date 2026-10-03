//! To-be-closed slots of the C stack (5.4+): `lua_toclose`,
//! `lua_closeslot`, and closing them as the stack shrinks or a C function
//! leaves.

use super::*;

/// Call `__close(v, err)` of the value in to-be-closed slot `i`; an error
/// in it replaces `err`.
fn close_one(api: &mut Api, i: usize, err: Option<Value>) -> Result<(), LuaError> {
    let v = api.stack()[i];
    if matches!(v, Value::Nil | Value::Bool(false)) {
        return Ok(());
    }
    let mm = api.vm.metafield(v, "__close");
    let e = err.unwrap_or(Value::Nil);
    api.vm.host_call(mm, &[v, e], None).map(drop)
}

/// Close every to-be-closed slot at or above C stack index `to`, top
/// first, with no error (`lua_settop`, `lua_pop`). An error a handler
/// raises is raised from the API call once the rest are closed with it.
pub(super) fn close_down_to(api: &mut Api, to: usize) {
    if let Err(e) = close_with(api, to, None) {
        api.raise(e);
    }
}

/// Close every to-be-closed slot at or above `to`, top first, passing
/// `err` (`None`: no error) on; the last error a handler raised wins.
pub(super) fn close_with(api: &mut Api, to: usize, err: Option<Value>) -> Result<(), LuaError> {
    let mut err = err;
    let mut failed = None;
    while let Some(&i) = api.st().tbc.last() {
        if i < to {
            break;
        }
        api.st().tbc.pop();
        if i >= api.top() {
            continue;
        }
        if let Err(e) = close_one(api, i, err) {
            err = Some(e.0);
            failed = Some(e);
        }
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// PUC `lua_toclose`: mark the stack slot `idx` to be closed when it
/// leaves the stack. The value must have a `__close` metamethod, or be nil
/// or false.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C wrapper, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_toclose`; the C
// wrapper `luna_c_lua_toclose` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_toclose(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let Some(i) = api.abs_stack(idx) else {
        return;
    };
    let v = api.stack()[i];
    if !matches!(v, Value::Nil | Value::Bool(false)) && api.vm.metafield(v, "__close").is_nil() {
        api.raise_msg("variable '?' got a non-closable value");
        return;
    }
    api.st().tbc.push(i);
}

/// PUC `lua_closeslot` (5.4.3+): close the to-be-closed slot `idx` and
/// set it to nil.
///
/// # Safety
/// As [`luna_capi_lua_toclose`].
// SAFETY: no other item in the link is named `luna_capi_lua_closeslot`; the
// C wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_closeslot(L: *mut LuaState, idx: c_int) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let Some(i) = api.abs_stack(idx) else {
        return;
    };
    if let Err(e) = close_with(&mut api, i, None) {
        api.raise(e);
        return;
    }
    api.stack_mut()[i] = Value::Nil;
}
