//! Protected calls: `pcall`, `xpcall` and the host-side xpcall.

use super::*;

/// `pcall` reached through `call_value`; a call from Lua goes through
/// `Vm::begin_pcall` instead, which makes the protected call yieldable.
pub(crate) fn nat_pcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let f = argcheck::check_any(vm, a, 0)?;
    let args: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    match vm.call_value(f, &args) {
        Ok(results) => {
            let mut out = Vec::with_capacity(results.len() + 1);
            out.push(Value::Bool(true));
            out.extend(results);
            Ok(vm.nat_return(fs, &out))
        }
        Err(e) => Ok(vm.nat_return(fs, &[Value::Bool(false), e.0])),
    }
}

/// `xpcall` reached through `call_value`; a call from Lua goes through
/// `Vm::begin_xpcall` instead, which makes the protected call yieldable.
pub(crate) fn nat_xpcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // 5.1 `xpcall(f, err)` calls `f` with no arguments.
    xpcall_native(vm, fs, nargs, vm.version() > LuaVersion::Lua51)
}

/// A host's protected call (`Vm::call_value_with_handler`): an `xpcall`
/// that passes its extra arguments on in every dialect, as `lua_pcall`
/// does, and is not a level of the stack. The dispatcher runs it as it
/// runs `xpcall`; this body only runs for a call that reaches it through
/// `call_value`.
pub(crate) fn nat_host_xpcall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    xpcall_native(vm, fs, nargs, true)
}

/// [`nat_host_xpcall`] made from inside a C function of the host's, which
/// is a level of the stack (`Vm::call_value_with_handler_in_c`). Natives
/// are told apart by address, so the body must not be one the compiler can
/// fold into `nat_host_xpcall`'s.
pub(crate) fn nat_host_xpcall_in_c(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    xpcall_native(vm, fs, nargs, std::hint::black_box(true))
}

fn xpcall_native(vm: &mut Vm, fs: u32, nargs: u32, forward: bool) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let h = xpcall_handler(vm, a)?;
    let f = vm.nat_arg(fs, nargs, 0);
    let first_arg = if forward { 2 } else { nargs };
    let args: Vec<Value> = (first_arg..nargs)
        .map(|i| vm.nat_arg(fs, nargs, i))
        .collect();
    match vm.call_value(f, &args) {
        Ok(results) => {
            let mut out = Vec::with_capacity(results.len() + 1);
            out.push(Value::Bool(true));
            out.extend(results);
            Ok(vm.nat_return(fs, &out))
        }
        Err(e) => {
            let m = handle_error(vm, h, e.0);
            Ok(vm.nat_return(fs, &[Value::Bool(false), m]))
        }
    }
}

/// `xpcall`'s check of its message handler: ≤5.2 only needs a second
/// argument; 5.3+ requires a function.
pub(crate) fn xpcall_handler(vm: &mut Vm, a: Args) -> Result<Value, LuaError> {
    if vm.version() >= LuaVersion::Lua53 {
        argcheck::check_function(vm, a, 1)
    } else {
        argcheck::check_any(vm, a, 1)
    }
}

/// Run `xpcall`'s message handler `h` on `err` for the `call_value` path.
/// On ≤5.2 a handler that is not a function cannot be called at all
/// (`luaG_errormsg` raises LUA_ERRERR), and an error inside the handler is
/// LUA_ERRERR too.
fn handle_error(vm: &mut Vm, h: Value, err: Value) -> Value {
    let callable =
        vm.version() >= LuaVersion::Lua53 || matches!(h, Value::Closure(_) | Value::Native(_));
    match callable.then(|| vm.call_value(h, &[err])) {
        Some(Ok(hr)) => hr.first().copied().unwrap_or(Value::Nil),
        _ => Value::Str(vm.heap.intern(b"error in error handling")),
    }
}
