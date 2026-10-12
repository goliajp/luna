//! The stack room a C function may ask for (`luaL_checkstack`).

use super::*;

/// 5.1 `LUAI_MAXCSTACK`: the most slots `lua_checkstack` grants a C
/// function.
pub(crate) const MAXCSTACK_51: i64 = 8000;

/// `luaL_checkstack` before a native pushes `n` more values.
#[inline]
pub(crate) fn check_stack(vm: &mut Vm, a: Args, n: i64, msg: &str) -> Result<(), LuaError> {
    // what fits below the plain limit fits whatever room an overflow adds
    if vm.version() != LuaVersion::Lua51
        && i64::from(a.fs + 1 + a.n) + n <= i64::from(vm.g.lua_stack_limit)
    {
        return Ok(());
    }
    check_stack_slow(vm, a, n, msg)
}

#[inline(never)]
fn check_stack_slow(vm: &mut Vm, a: Args, n: i64, msg: &str) -> Result<(), LuaError> {
    let fits = if vm.version() == LuaVersion::Lua51 {
        n <= MAXCSTACK_51 && i64::from(a.n) + n <= MAXCSTACK_51
    } else {
        vm.checkstack(a.fs + 1 + a.n, n)
    };
    if fits {
        Ok(())
    } else {
        Err(raise_str(vm, &format!("stack overflow ({msg})")))
    }
}
