//! `coroutine.wrap` and the function it returns.

use super::*;
use crate::vm::exec::SpecialErrors;

/// The function returned by `coroutine.wrap`: upvalue [0] holds the coroutine;
/// resuming it propagates errors instead of returning `(false, err)`. What
/// comes before and after the resume are functions of their own: in an
/// unoptimised build their locals are then off the stack while the
/// coroutine runs, which a chain of wrapped calls repeats on every level.
fn co_wrapped(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (before, go) = wrap_args(vm, fs, nargs)?;
    let err = match go {
        Ok((co, in_wrap, args)) => {
            let r = vm.resume_coro(co, args);
            match wrap_results(vm, fs, co, in_wrap, r)? {
                Ok(n) => return Ok(n),
                Err(err) => err,
            }
        }
        Err(refused) => refused,
    };
    // 5.4+ leave a memory error as it is
    if vm.version() >= LuaVersion::Lua54
        && vm.error_status(err, before) == crate::runtime::ErrorStatus::Mem
    {
        return Err(LuaError(err));
    }
    Err(LuaError(wrap_where(vm, err)))
}

/// What a resume replaces in the counts of special errors, and the
/// coroutine to resume with its arguments, or the error of a refusal.
#[allow(clippy::type_complexity)]
#[cfg_attr(luna_hot, inline(always))]
fn wrap_args(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
) -> Result<
    (
        SpecialErrors,
        Result<(Gc<Coro>, Gc<Table>, Vec<Value>), Value>,
    ),
    LuaError,
> {
    let Value::Coro(co) = vm.nat_upval(fs, 0) else {
        unreachable!("wrap upvalue is a coroutine");
    };
    let in_wrap = upval_table(vm, fs, 1);
    // the arguments move to the coroutine, and its error comes back pushed
    let args = collect_args(vm, fs, nargs);
    vm.native_settop(0);
    let before = vm.special_errors();
    let go = match resume_refusal(vm, co, nargs == 0, in_wrap) {
        Some(msg) => {
            let m = refusal_value(vm, &msg)?;
            vm.native_push_vals(&[m])?;
            Err(m)
        }
        None => {
            mark_in_wrap(vm, in_wrap, true)?;
            Ok((co, in_wrap, args))
        }
    };
    Ok((before, go))
}

/// The wrapped function's results for the coroutine's `r`, or the error it
/// raises.
#[cfg_attr(luna_hot, inline(always))]
fn wrap_results(
    vm: &mut Vm,
    fs: u32,
    co: Gc<Coro>,
    in_wrap: Gc<Table>,
    r: Result<Vec<Value>, LuaError>,
) -> Result<Result<u32, Value>, LuaError> {
    mark_in_wrap(vm, in_wrap, false)?;
    if let Err(e) = &r {
        vm.native_push_vals(&[e.0])?;
    }
    Ok(match r {
        Ok(vals) => return vm.nat_ret(fs, &vals).map(Ok),
        // 5.4+ close a coroutine that died by error before
        // re-raising, so its pending `__close` handlers run (and one
        // of them may replace the error), whose error is pushed over
        // the first; one the resume refused to start ("C stack
        // overflow") is not dead and stays open
        Err(_) if vm.version() >= LuaVersion::Lua54 && co.status == CoroStatus::Dead => {
            vm.native_push(1);
            match vm.close_coro(co) {
                Ok(Some(e)) => Err(death_value(vm, e)),
                Ok(None) => unreachable!("a coroutine that died by error has an error"),
                Err(e) => Err(death_value(vm, e.0)),
            }
        }
        Err(e) => Err(death_value(vm, e.0)),
    })
}

/// `luaB_auxwrap` prefixes a string error with `luaL_where(L, 1)` — the
/// position of whoever called the wrapped function. ≤5.2 test `lua_isstring`,
/// so a number error is converted and prefixed too; 5.3+ only strings.
fn wrap_where(vm: &mut Vm, err: Value) -> Value {
    match err {
        Value::Str(_) => {}
        Value::Int(_) | Value::Float(_) if vm.version() <= LuaVersion::Lua52 => {}
        _ => return err,
    }
    let pos = vm.position_at_level(1);
    let r = crate::vm::builtins::lual_where(vm, pos)
        .and_then(|at| crate::vm::builtins::concat2(vm, Value::Str(at), err));
    r.unwrap_or_else(|e| e.0)
}

pub(super) fn co_wrap<const PRE: bool>(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let co = new_body_coro(vm, Args::new(fs, nargs))?;
    let in_wrap = vm.nat_upval(fs, 0);
    // PUC's function has the coroutine alone
    let f = vm.lib_native_puc::<PRE>(co_wrapped, &[Value::Coro(co), in_wrap], 1)?;
    vm.nat_ret1(fs, f)
}
