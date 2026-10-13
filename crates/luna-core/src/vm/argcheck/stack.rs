//! The stack room a C function may ask for (`luaL_checkstack`).

use super::*;

/// 5.1 `LUAI_MAXCSTACK`: the most slots `lua_checkstack` grants a C
/// function.
pub(crate) const MAXCSTACK_51: i64 = 8000;

/// `luaL_checkstack` before a native pushes `n` more values.
#[inline]
pub(crate) fn check_stack(vm: &mut Vm, a: Args, n: i64, msg: &str) -> Result<(), LuaError> {
    // what fits under the stack's size needs no look at it, except the
    // room granted, which the C function's frame reserves from then on
    let top = a.fs + 1 + a.n;
    let room = i64::from(vm.g.stack_last.saturating_sub(top));
    if vm.version() != LuaVersion::Lua51 && (0..room).contains(&n) {
        vm.reserve_native_room(top + n as u32);
        return Ok(());
    }
    check_stack_slow(vm, a, n, msg)
}

#[inline(never)]
fn check_stack_slow(vm: &mut Vm, a: Args, n: i64, msg: &str) -> Result<(), LuaError> {
    let fits = vm.checkstack(a.fs + 1 + a.n, n);
    if fits {
        Ok(())
    } else {
        Err(crate::vm::builtins::raise_fmt(
            vm,
            1,
            &format!("stack overflow ({msg})"),
        ))
    }
}
