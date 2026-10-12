//! A C call's results on the stack, and the status of a protected call's
//! error.

use super::*;

/// Put a call's results on the stack from index `at`, where its function
/// was, adjusted to `nresults` (-1: all). The results may be held by
/// nothing else, so no emergency collection runs: a memory error comes
/// back when the stack cannot grow for them.
pub(in crate::capi) fn push_results(
    vm: &mut Vm,
    co: Gc<Coro>,
    at: usize,
    mut vals: Vec<Value>,
    nresults: c_int,
) -> Result<(), LuaError> {
    if nresults >= 0 {
        vals.resize(nresults as usize, Value::Nil);
    }
    cstack(co).truncate(at);
    cstack(co).extend_from_slice(&vals)?;
    vm.heap.barrier_back(co);
    Ok(())
}

/// What a protected call's `true, results...` or `false, error` leaves on
/// the stack from `at`: `None` for success, else the error status.
pub(in crate::capi) fn pcall_outcome(
    vm: &mut Vm,
    co: Gc<Coro>,
    mut vals: Vec<Value>,
    nresults: c_int,
    at: usize,
    errerr_before: SpecialErrors,
) -> Option<c_int> {
    let ok = vals.first().is_some_and(|v| v.truthy());
    if ok {
        vals.remove(0);
        let Err(e) = push_results(vm, co, at, vals, nresults) else {
            return None;
        };
        cstack(co).truncate(at);
        cstack(co).push_or_abort(e.0);
        return Some(LUA_ERRMEM);
    }
    let err = vals.get(1).copied().unwrap_or(Value::Nil);
    cstack(co).truncate(at);
    cstack(co).push_or_abort(err);
    let kind = vm.error_status(err, errerr_before);
    Some(status_code(vm.version(), kind))
}

/// The status code of the dialect for an error of status `kind`.
pub(in crate::capi) fn status_code(v: LuaVersion, kind: ErrorStatus) -> c_int {
    let v52 = matches!(v, LuaVersion::Lua52 | LuaVersion::Lua53);
    match kind {
        ErrorStatus::Run => LUA_ERRRUN,
        ErrorStatus::Mem => LUA_ERRMEM,
        ErrorStatus::Err if v52 => 6,
        ErrorStatus::Err => LUA_ERRERR,
        // 5.2/5.3 LUA_ERRGCMM: a finalizer failed in a full collection
        ErrorStatus::Gcmm => 5,
    }
}
