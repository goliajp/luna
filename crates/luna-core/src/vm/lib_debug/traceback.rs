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
    if !dead_with_levels(co) {
        let s = vm.puc_traceback(co, msg.as_deref(), level)?;
        return vm.nat_ret(fs, &[Value::Str(s)]);
    }
    let mut out = match msg {
        Some(mut m) => {
            m.push(b'\n');
            m
        }
        None => Vec::new(),
    };
    out.extend_from_slice(b"stack traceback:");
    out.extend(thread_traceback(vm, co, level));
    let s = vm.str_value(&out)?;
    vm.nat_ret(fs, &[s])
}

/// A coroutine killed by an error, whose stack luna keeps as text.
fn dead_with_levels(co: Option<Gc<Coro>>) -> bool {
    co.is_some_and(|c| c.error_levels.is_some())
}

/// The level lines of `co`'s traceback; a coroutine killed by an error
/// shows the stack it died with, as PUC leaves a dead thread's stack.
fn thread_traceback(vm: &mut Vm, co: Option<Gc<Coro>>, level: i64) -> Vec<u8> {
    if let Some(co) = co
        && let Some(lines) = co.error_levels.as_ref()
    {
        return crate::vm::callstack::traceback_from_lines(vm.version(), lines, level, 0);
    }
    vm.traceback_lines(co, level)
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
    if !dead_with_levels(co) {
        let r = vm.puc_errorfb(co, stack, level)?;
        return vm.nat_ret(fs, &[r]);
    }
    let mut out = Vec::new();
    if let Some(&msg) = stack.first() {
        if crate::vm::argcheck::to_str_bytes(vm, msg).is_none() {
            // `return 1` hands back whatever is on top
            let top = *stack.last().expect("non-empty");
            return vm.nat_ret(fs, &[top]);
        }
        // `lua_concat` works from the top down, so the rightmost bad part is
        // the one reported; the C function has no position to add
        if let Some(bad) = stack
            .iter()
            .rev()
            .find(|&&p| crate::vm::argcheck::to_str_bytes(vm, p).is_none())
        {
            let msg = format!("attempt to concatenate a {} value", bad.type_name());
            return Err(vm.runerror_in_c(&msg));
        }
        for &part in &stack {
            out.extend(crate::vm::argcheck::to_str_bytes(vm, part).expect("checked above"));
        }
        out.push(b'\n');
    }
    out.extend_from_slice(b"stack traceback:");
    out.extend(thread_traceback(vm, co, level));
    let s = vm.str_value(&out)?;
    vm.nat_ret(fs, &[s])
}
