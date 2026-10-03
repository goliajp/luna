//! `debug.getlocal` and `debug.setlocal`.

use super::{check_int, getthread, is_function};
use crate::runtime::{Coro, Gc, LuaClosure, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, check_any};
use crate::vm::builtins::arg_error;
use crate::vm::callstack::LocalSlot;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// `lua_getstack` for the local-variable functions: the level index
/// (`None`: 5.1's answer to a negative level, a lost tail call, which has
/// no locals), or the "level out of range" argument error.
fn stack_level(
    vm: &mut Vm,
    co: Option<Gc<Coro>>,
    level: i64,
    argn: u32,
) -> Result<Option<usize>, LuaError> {
    let n = vm.thread_stack(co).levels.len();
    match usize::try_from(level) {
        Ok(i) if i < n => Ok(Some(i)),
        _ if level < 0 && vm.version() == LuaVersion::Lua51 => Ok(None),
        _ => Err(arg_error(vm, argn, "level out of range")),
    }
}

fn read_local(vm: &Vm, co: Option<Gc<Coro>>, at: LocalSlot) -> Value {
    match at {
        LocalSlot::Held(v) => v,
        LocalSlot::Stack(slot) => {
            let ts = vm.thread_stack(co);
            // luna sizes the value stack lazily; a slot past it holds nil
            ts.stack.get(slot).copied().unwrap_or(Value::Nil)
        }
    }
}

fn find_local(
    vm: &Vm,
    co: Option<Gc<Coro>>,
    level: Option<usize>,
    n: i64,
) -> Option<(String, LocalSlot)> {
    let ts = vm.thread_stack(co);
    vm.find_local(&ts, level?, n)
}

pub(super) fn d_getlocal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let (co, arg) = getthread(vm, a);
    if v == LuaVersion::Lua51 {
        let level = check_int(vm, a, arg)?;
        let level = stack_level(vm, co, level, arg + 1)?;
        let n = check_int(vm, a, arg + 1)?;
        return getlocal_result(vm, fs, co, level, n);
    }
    let n = check_int(vm, a, arg + 1)?;
    if is_function(vm, a, arg) {
        // the name of parameter `n`, from the prototype
        let name = match a.get(vm, arg) {
            Value::Closure(cl) => param_name(cl, n),
            _ => None,
        };
        let r = match name {
            Some(nm) => Value::Str(vm.heap.intern(nm.as_bytes())),
            None => Value::Nil,
        };
        return Ok(vm.nat_return(fs, &[r]));
    }
    let level = check_int(vm, a, arg)?;
    let level = stack_level(vm, co, level, arg + 1)?;
    getlocal_result(vm, fs, co, level, n)
}

fn getlocal_result(
    vm: &mut Vm,
    fs: u32,
    co: Option<Gc<Coro>>,
    level: Option<usize>,
    n: i64,
) -> Result<u32, LuaError> {
    match find_local(vm, co, level, n) {
        Some((name, at)) => {
            let val = read_local(vm, co, at);
            let nm = Value::Str(vm.heap.intern(name.as_bytes()));
            Ok(vm.nat_return(fs, &[nm, val]))
        }
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

/// PUC `luaF_getlocalname(p, n, 0)`: the `n`-th local live at the first
/// instruction — a parameter.
pub(crate) fn param_name(cl: Gc<LuaClosure>, n: i64) -> Option<String> {
    let mut live: Vec<&crate::runtime::LocVar> = cl
        .proto
        .locvars
        .iter()
        .filter(|lv| lv.start_pc == 0 && lv.end_pc > 0)
        .collect();
    live.sort_by_key(|lv| lv.reg);
    let lv = live.get(usize::try_from(n.checked_sub(1)?).ok()?)?;
    Some(lv.name.to_string())
}

pub(super) fn d_setlocal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (co, arg) = getthread(vm, a);
    let (level, n) = if vm.version() <= LuaVersion::Lua52 {
        let level = check_int(vm, a, arg)?;
        let level = stack_level(vm, co, level, arg + 1)?;
        check_any(vm, a, arg + 2)?;
        (level, check_int(vm, a, arg + 1)?)
    } else {
        let level = check_int(vm, a, arg)?;
        let n = check_int(vm, a, arg + 1)?;
        let level = stack_level(vm, co, level, arg + 1)?;
        check_any(vm, a, arg + 2)?;
        (level, n)
    };
    let val = a.get(vm, arg + 2);
    let name = match find_local(vm, co, level, n) {
        Some((name, LocalSlot::Stack(slot))) => {
            vm.write_thread_slot(co, slot, val);
            Some(name)
        }
        // the value lives in a slot of PUC's C function that luna's native
        // does not have; nothing to write
        Some((name, LocalSlot::Held(_))) => Some(name),
        None => None,
    };
    let r = match name {
        Some(nm) => Value::Str(vm.heap.intern(nm.as_bytes())),
        None => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[r]))
}
