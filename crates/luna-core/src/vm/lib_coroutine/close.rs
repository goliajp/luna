//! `coroutine.close`.

use super::*;

pub(crate) fn co_close(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // 5.5 `getoptco`: with no argument, close the running thread itself.
    let co = if vm.version() >= LuaVersion::Lua55 && a.is_none(0) {
        match vm.current_coro() {
            Some(c) => c,
            None => return Err(main_close_err(vm)),
        }
    } else {
        check_co(vm, a)?
    };
    // PUC 5.4 `auxstatus` reports a coroutine as "running" when it is the
    // currently-executing thread — that path errors with "cannot close a
    // running coroutine". 5.5 instead lets the re-entrant call succeed (the
    // outer close finishes the work). The condition is the same as luna's
    // close_coro re-entrant guard.
    if vm.version() < LuaVersion::Lua55 && vm.current_coro().is_some_and(|c| c.ptr_eq(co)) {
        return Err(raise_str(vm, "cannot close a running coroutine"));
    }
    match vm.effective_coro_status(co) {
        CoroStatus::Dead | CoroStatus::Suspended => match vm.close_coro(co) {
            // died with an error, or a __close handler raised: report (false, e)
            Ok(Some(e)) => {
                let e = death_value(vm, e);
                Ok(vm.nat_return(fs, &[Value::Bool(false), e]))
            }
            Ok(None) => Ok(vm.nat_return(fs, &[Value::Bool(true)])),
            Err(e) => {
                let e = death_value(vm, e.0);
                Ok(vm.nat_return(fs, &[Value::Bool(false), e]))
            }
        },
        CoroStatus::Normal => Err(raise_str(vm, "cannot close a normal coroutine")),
        CoroStatus::Running => {
            // 5.5 refuses the main thread by name and lets a running thread
            // close *itself* by running its to-be-closed handlers in place;
            // 5.4 rolls both into "cannot close a running coroutine".
            if vm.version() >= LuaVersion::Lua55 {
                if vm.is_main_coro(co) {
                    return Err(main_close_err(vm));
                }
                if vm.current_coro().is_some_and(|c| c.ptr_eq(co)) {
                    return Err(vm.close_running());
                }
            }
            Err(raise_str(vm, "cannot close a running coroutine"))
        }
    }
}

/// over the main thread `lua_geti` fetched from the registry to compare
pub(crate) fn main_close_err(vm: &mut Vm) -> LuaError {
    vm.native_push(1);
    raise_str(vm, "cannot close main thread")
}
