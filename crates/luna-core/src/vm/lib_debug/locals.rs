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

/// The source string of the chunk of the Lua function at `level`: its
/// locals' names are that chunk's.
fn level_source(
    vm: &Vm,
    co: Option<Gc<Coro>>,
    level: Option<usize>,
) -> Option<Gc<crate::runtime::LuaStr>> {
    let ts = vm.thread_stack(co);
    match *ts.levels.get(level?)? {
        crate::vm::callstack::DbgKind::Lua(fi) => Some(ts.lua(fi).closure.proto.source),
        _ => None,
    }
}

pub(super) fn d_getlocal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if !vm.reader_layers.is_empty() {
        vm.fill_reader_layers()?;
    }
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
        let src = match a.get(vm, arg) {
            Value::Closure(cl) => Some(cl.proto.source),
            _ => None,
        };
        let r = match name {
            Some(nm) => super::push_name(vm, src, nm.as_bytes())?,
            None => Value::Nil,
        };
        return vm.nat_ret(fs, &[r]);
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
            let src = level_source(vm, co, level);
            let nm = super::push_name(vm, src, name.as_bytes())?;
            vm.nat_ret(fs, &[nm, val])
        }
        None => vm.nat_ret(fs, &[Value::Nil]),
    }
}

/// PUC `luaF_getlocalname(p, n, 0)`: the `n`-th local live at the first
/// instruction — a fixed parameter (5.5's vararg parameter starts after
/// `VARARGPREP`, which luna's code does not have).
pub(crate) fn param_name(cl: Gc<LuaClosure>, n: i64) -> Option<String> {
    let np = u32::from(cl.proto.num_params);
    let mut live: Vec<&crate::runtime::LocVar> = cl
        .proto
        .locvars
        .iter()
        .filter(|lv| lv.start_pc == 0 && lv.end_pc > 0 && lv.reg < np)
        .collect();
    live.sort_by_key(|lv| lv.reg);
    let lv = live.get(usize::try_from(n.checked_sub(1)?).ok()?)?;
    Some(lv.name.to_string())
}

pub(super) fn d_setlocal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if !vm.reader_layers.is_empty() {
        vm.fill_reader_layers()?;
    }
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
            vm.write_thread_slot(co, slot, &name, val)?;
            Some(name)
        }
        // the value lives in a slot of PUC's C function that luna's native
        // does not have; nothing to write
        Some((name, LocalSlot::Held(_))) => Some(name),
        None => None,
    };
    let src = level_source(vm, co, level);
    let r = match name {
        Some(nm) => super::push_name(vm, src, nm.as_bytes())?,
        None => Value::Nil,
    };
    vm.nat_ret(fs, &[r])
}
