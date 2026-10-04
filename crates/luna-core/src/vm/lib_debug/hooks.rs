//! `debug.sethook` and `debug.gethook`.

use super::getthread;
use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, check_function, check_string, opt_integer};
use crate::vm::error::LuaError;
use crate::vm::exec::{HookState, Vm};

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
    vm.set_hook(co, state);
    Ok(0)
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
    let hook = match state.func {
        _ if armed && external => Value::Str(vm.heap.intern(b"external hook")),
        Some(h) if armed => h,
        _ if v >= LuaVersion::Lua54 => return Ok(vm.nat_return(fs, &[Value::Nil])),
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
    let m = Value::Str(vm.heap.intern(&mask));
    Ok(vm.nat_return(fs, &[hook, m, Value::Int(state.count_base)]))
}
