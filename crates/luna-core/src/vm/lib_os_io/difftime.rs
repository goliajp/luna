//! `os.difftime`, `os.clock`, and the time argument they share with
//! `os.date`.

use super::*;

/// `l_checktime` (5.3+) or ≤5.2's `(time_t)luaL_checknumber`.
pub(super) fn check_time(vm: &mut Vm, a: Args, i: u32) -> Result<i64, LuaError> {
    if vm.version() <= LuaVersion::Lua52 {
        return Ok(c_abi::to_time(argcheck::check_number(vm, a, i)?));
    }
    let t = argcheck::check_integer(vm, a, i)?;
    if !c_abi::is_time(t) {
        return Err(arg_error(vm, i + 1, "time out-of-bounds"));
    }
    Ok(t)
}

pub(super) fn os_difftime(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // ≤5.2 check both in one call's arguments (in the order the compiler
    // evaluates them) and default the second time to 0; 5.3 made it
    // required
    if c_abi::ARGS_RTL && vm.version() <= LuaVersion::Lua52 {
        let t2 = c_abi::to_time(argcheck::opt_number(vm, a, 1, 0.0)?);
        let t1 = check_time(vm, a, 0)?;
        return vm.nat_ret1(fs, Value::Float(t1 as f64 - t2 as f64));
    }
    let t1 = check_time(vm, a, 0)?;
    let t2 = if vm.version() <= LuaVersion::Lua52 {
        c_abi::to_time(argcheck::opt_number(vm, a, 1, 0.0)?)
    } else {
        check_time(vm, a, 1)?
    };
    vm.nat_ret1(fs, Value::Float(t1 as f64 - t2 as f64))
}

pub(super) fn os_clock(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    // C's clock() is processor time, which std cannot read; the time since
    // the Vm started stands in for it.
    let secs = vm.uptime().as_secs_f64();
    vm.nat_ret1(fs, Value::Float(secs))
}
