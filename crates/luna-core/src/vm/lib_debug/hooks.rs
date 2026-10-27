//! `debug.sethook` and `debug.gethook`.

use super::getthread;
use crate::runtime::{Gc, Table, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, check_function, check_string, opt_integer};
use crate::vm::error::LuaError;
use crate::vm::exec::{HOOK_KEY, HookState, Vm, light_key};

pub(super) fn d_sethook(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (co, arg) = getthread(vm, a);
    let state = if a.is_none_or_nil(vm, arg) {
        HookState::default()
    } else {
        let mask = check_string(vm, a, arg + 1)?.as_bytes().to_vec();
        let func = check_function(vm, a, arg)?;
        let count = opt_integer(vm, a, arg + 2, 0)? as i32 as i64;
        HookState {
            func: Some(func),
            rust_func: None,
            call: mask.contains(&b'c'),
            ret: mask.contains(&b'r'),
            line: mask.contains(&b'l'),
            count: count > 0,
            count_base: count,
            count_left: count,
        }
    };
    // the registry's hook table records the function for the thread
    let t = hook_table(vm, true)?;
    let key = thread_key(vm, co);
    let f = a.get(vm, arg);
    vm.raw_set(t, key, f)?;
    vm.set_hook(co, state);
    Ok(0)
}

/// The hook table in the registry, made when it is not there: 5.1
/// `gethooktable`, later `luaL_getsubtable` (5.3 `lua_rawgetp`). `init`:
/// `sethook` gives a table it makes from 5.2 on weak keys, with itself as
/// its metatable.
fn hook_table(vm: &mut Vm, init: bool) -> Result<Gc<Table>, LuaError> {
    let v = vm.version();
    let reg = vm.host_registry();
    let key = match v {
        LuaVersion::Lua51 | LuaVersion::Lua53 => light_key(&HOOK_KEY),
        LuaVersion::Lua52 => vm.str_value(b"_HKEY")?,
        _ => vm.str_value(b"_HOOKKEY")?,
    };
    if let Value::Table(t) = reg.get(key) {
        return Ok(t);
    }
    let t = match v {
        LuaVersion::Lua51 => vm.api_new_table(0, 1)?,
        LuaVersion::Lua53 => vm.api_new_table(0, 2)?,
        _ => vm.api_new_table(0, 0)?,
    };
    // `lua_setfield` makes the key's string again: the one read with
    // `lua_getfield` may have been collected by the table's allocation
    let key = match key {
        Value::Str(_) => vm.str_value(if v == LuaVersion::Lua52 {
            b"_HKEY"
        } else {
            b"_HOOKKEY"
        })?,
        k => k,
    };
    vm.raw_set(reg, key, Value::Table(t))?;
    if init && v >= LuaVersion::Lua52 {
        let k = vm.api_push_string(b"k")?;
        let mode = vm.str_value(b"__mode")?;
        vm.raw_set(t, mode, k)?;
        vm.host_set_metatable(Value::Table(t), Some(t))?;
    }
    Ok(t)
}

/// The key of a thread in the hook table: 5.1 its address, later the
/// thread.
fn thread_key(vm: &mut Vm, co: Option<Gc<crate::runtime::Coro>>) -> Value {
    let co = co
        .or_else(|| vm.current_coro())
        .unwrap_or_else(|| vm.host_main_thread());
    if vm.version() == LuaVersion::Lua51 {
        Value::LightUserdata(co.as_ptr() as *const ())
    } else {
        Value::Coro(co)
    }
}

pub(super) fn d_gethook(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (co, _) = getthread(vm, Args::new(fs, nargs));
    let v = vm.version();
    let state = vm.get_hook(co);
    // `lua_sethook` drops a hook whose mask is empty; 5.1/5.2 still report
    // the function the hook table recorded for the thread
    let armed = state.call || state.ret || state.line || state.count;
    // a C hook (`lua_sethook`) keeps its function as a light userdata
    let external = matches!(state.func, None | Some(Value::LightUserdata(_)));
    // 5.1 and 5.2 look the function up in the hook table, making it
    if v <= LuaVersion::Lua52 && !(armed && external) {
        hook_table(vm, false)?;
    }
    let hook = match state.func {
        _ if armed && external => vm.api_push_string(b"external hook")?,
        Some(h) if armed => h,
        _ if v >= LuaVersion::Lua54 => return vm.nat_ret(fs, &[Value::Nil]),
        Some(h) if v <= LuaVersion::Lua52 => h,
        _ => Value::Nil,
    };
    let mut mask = Vec::new();
    if state.call {
        mask.push(b'c');
    }
    if state.ret {
        mask.push(b'r');
    }
    if state.line {
        mask.push(b'l');
    }
    // the hook is on PUC's stack while the mask is made
    let m = vm.rooted(&[hook], |vm| vm.api_push_string(&mask))?;
    vm.nat_ret(fs, &[hook, m, Value::Int(state.count_base)])
}
