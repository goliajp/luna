//! `debug.traceback`, including 5.1's own layout.

use super::getthread;
use crate::runtime::{Coro, Gc, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, opt_integer, to_num};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(super) fn d_traceback(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (co, arg) = getthread(vm, a);
    let default_level = if co.is_none() || vm.is_current_thread(co) {
        1
    } else {
        0
    };
    if vm.version() == LuaVersion::Lua51 {
        return traceback_51(vm, fs, a, co, arg, default_level);
    }
    // `lua_tostring`: a number message becomes a string in its slot
    let msg = crate::vm::argcheck::tolstring_in_place(vm, a, arg)?.map(|s| s.as_bytes().to_vec());
    if msg.is_none() && !a.is_none_or_nil(vm, arg) {
        // a message that is not a string is returned untouched
        let m = a.get(vm, arg);
        return vm.nat_ret(fs, &[m]);
    }
    let level = opt_integer(vm, a, arg + 1, default_level)? as i32 as i64;
    let s = vm.puc_traceback(co, msg.as_deref(), level)?;
    vm.nat_ret(fs, &[Value::Str(s)])
}

/// PUC 5.1 `db_errorfb`, which works on the raw argument stack: a numeric
/// last-but-one argument is the level (and is popped from the *top*), a
/// non-string message is returned as is, and every argument still on the
/// stack is concatenated in front of the traceback.
fn traceback_51(
    vm: &mut Vm,
    fs: u32,
    a: Args,
    co: Option<Gc<Coro>>,
    arg: u32,
    default_level: i64,
) -> Result<u32, LuaError> {
    let mut stack: Vec<Value> = (arg..a.n).map(|i| a.get(vm, i)).collect();
    let level = match stack.get(1).and_then(|&v| to_num(vm, v)) {
        Some(n) => {
            stack.pop();
            // `db_errorfb` keeps the level in a C `int`
            crate::vm::c_abi::tointeger52(LuaVersion::Lua51, n) as i32 as i64
        }
        None => default_level,
    };
    if let Some(&msg) = stack.first()
        && crate::vm::argcheck::to_str_bytes(vm, msg).is_none()
    {
        // `return 1` hands back whatever is on top
        let top = *stack.last().expect("non-empty");
        return vm.nat_ret(fs, &[top]);
    }
    let r = vm.puc_errorfb(co, stack, level)?;
    vm.nat_ret(fs, &[r])
}
