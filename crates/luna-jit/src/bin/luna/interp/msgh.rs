//! `lua.c`'s message handler.

use super::*;
use luna_core::__private::{VmCApiValues, VmHostInternals};

/// `lua.c`'s message handler of each dialect: the error message with a
/// traceback of where it was raised (level 1 skips the handler itself).
pub(super) fn msghandler(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let err = vm.nat_arg(fs, nargs, 0);
    let out = match vm.version() {
        LuaVersion::Lua51 => traceback_51(vm, err)?,
        LuaVersion::Lua52 => match lua_tostring(vm, err) {
            Some(msg) => traceback(vm, &msg)?,
            None if err.is_nil() => err,
            // `luaL_callmeta`: whatever `__tostring` returns
            None => match vm.metafield(err, "__tostring") {
                Value::Nil => Value::Str(vm.heap.intern(b"(no error message)")),
                mm => vm
                    .call_value(mm, &[err])?
                    .first()
                    .copied()
                    .unwrap_or(Value::Nil),
            },
        },
        _ => match lua_tostring(vm, err) {
            Some(msg) => traceback(vm, &msg)?,
            None => {
                let mm = vm.metafield(err, "__tostring");
                let text = if mm.is_nil() {
                    None
                } else {
                    vm.call_value(mm, &[err])?.first().copied()
                };
                match text {
                    // a string from `__tostring` is the message, as it is
                    Some(s @ Value::Str(_)) => s,
                    _ => {
                        let msg = format!("(error object is a {} value)", err.type_name());
                        traceback(vm, msg.as_bytes())?
                    }
                }
            }
        },
    };
    Ok(vm.nat_return(fs, &[out]))
}

/// `luaL_traceback(L, L, msg, 1)` from the message handler.
fn traceback(vm: &mut Vm, msg: &[u8]) -> Result<Value, LuaError> {
    vm.traceback_string(Some(msg), 1)
}

/// 5.1 lua.c's `traceback`: a string (or number) message goes through the
/// global `debug.traceback(msg, 2)` when there is one; anything else, or no
/// such function, leaves the message as it is.
fn traceback_51(vm: &mut Vm, err: Value) -> Result<Value, LuaError> {
    if !matches!(err, Value::Str(_) | Value::Int(_) | Value::Float(_)) {
        return Ok(err);
    }
    let key = Value::Str(vm.heap.intern(b"debug"));
    let Value::Table(debug) = vm.globals().get(key) else {
        return Ok(err);
    };
    let key = Value::Str(vm.heap.intern(b"traceback"));
    let tb = debug.get(key);
    if !matches!(tb, Value::Closure(_) | Value::Native(_)) {
        return Ok(err);
    }
    Ok(vm
        .call_value(tb, &[err, Value::Int(2)])?
        .first()
        .copied()
        .unwrap_or(Value::Nil))
}

/// 5.1's `lua_pushcfunction` of the handler in `docall` checks for a
/// collection, the chunk `f` and its `args` on the stack.
pub(super) fn check_handler_push(vm: &mut Vm, f: Value, args: &[Value]) -> Result<(), Value> {
    if vm.version() != LuaVersion::Lua51 {
        return Ok(());
    }
    let held: Vec<_> = std::iter::once(f)
        .chain(args.iter().copied())
        .map(|v| vm.pin_host(v))
        .collect();
    let checked = vm.host_api_check(true);
    for t in held {
        let _ = vm.unpin(t);
    }
    checked.map_err(|e| e.0)
}
