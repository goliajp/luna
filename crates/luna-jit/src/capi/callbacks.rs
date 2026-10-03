//! C function callbacks and the version probe.

use super::*;

/// Rust-side trampoline that any `lua_pushcfunction`-registered C function
/// is wrapped in. Its upvalue slot 0 holds the C function pointer as a
/// `LightUserdata`. The bridge:
///   1. mirrors the Vm dispatch frame's args into `vm.capi_stack` (so the
///      C callback's `lua_tointeger(L, 1)` etc. resolve to its arguments)
///   2. demotes `&mut Vm` to a raw `*mut Vm`, casts that to `*mut LuaState`
///      (sound because `LuaState` is `#[repr(transparent)] Vm`), and calls
///      the C function
///   3. takes the top `nret` values back off `capi_stack` and returns them
///      via `nat_return`
///
/// Aliasing note: the `&mut Vm` reference is held until just before
/// `cf(L_ptr)`; the raw pointer cast drops the unique-reference invariant.
/// During the C call we do NOT touch `vm` through the (stale) reference —
/// only via the raw pointer the C side now exclusively owns. After the C
/// callback returns we re-borrow from the raw pointer, which is sound
/// because no other live reference exists at that point.
fn capi_trampoline(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let cf_value = vm.running_native_upvalue(0);
    let cf: LuaCFunction = match cf_value {
        // SAFETY: source and destination types share the same in-memory representation; see the C ABI typedef this function implements.
        Value::LightUserdata(p) => unsafe { std::mem::transmute::<*const (), LuaCFunction>(p) },
        _ => {
            let s = Value::Str(vm.heap.intern(b"missing C function pointer upvalue"));
            return Err(LuaError(s));
        }
    };
    // Mirror args from the Vm dispatch frame to the C-visible capi_stack
    // (via the public `nat_arg` accessor — works for the missing-arg-is-nil
    // contract too).
    // the C function's frame: its index 1 is its first argument
    let baseline = vm.capi_stack.len();
    let outer_base = std::mem::replace(&mut vm.capi_base, baseline);
    for i in 0..nargs {
        let v = vm.nat_arg(fs, nargs, i);
        vm.capi_stack.push(v);
    }
    vm.capi_calls += 1;
    // Demote to raw pointer; the &mut Vm is no longer live across the cf call.
    let vm_ptr: *mut Vm = vm as *mut Vm;
    let nret = cf(vm_ptr as *mut LuaState) as usize;
    // Re-borrow.
    // SAFETY: `vm_ptr` came from the `&mut Vm` this function holds for its whole call, and the C
    // function has returned, so no reference it made from `L` is still in use
    let vm = unsafe { &mut *vm_ptr };
    vm.capi_calls -= 1;
    vm.capi_base = outer_base;
    if let Some(e) = vm.capi_error.take() {
        vm.capi_stack.truncate(baseline);
        return Err(LuaError(e));
    }
    let stack_len = vm.capi_stack.len();
    if stack_len < baseline + nret {
        // C function lied about its return count.
        let s = Value::Str(
            vm.heap
                .intern(b"C function returned more values than were pushed"),
        );
        vm.capi_stack.truncate(baseline);
        return Err(LuaError(s));
    }
    let results_start = stack_len - nret;
    let results: Vec<Value> = vm.capi_stack[results_start..].to_vec();
    vm.capi_stack.truncate(baseline);
    Ok(vm.nat_return(fs, &results))
}

/// Push a C function as a Lua callable on the stack. The C function receives
/// the calling `LuaState*` and reads its args from positions 1..N on the
/// stack; it must push its results and return the result count (PUC's
/// `lua_pushcfunction`).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pushcfunction`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushcfunction(L: *mut LuaState, f: LuaCFunction) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    let cf_ptr = f as *const ();
    let trampoline: luna_core::runtime::value::NativeFn = capi_trampoline;
    let f_val = vm.native_with(trampoline, Box::new([Value::LightUserdata(cf_ptr)]));
    vm.capi_stack.push(f_val);
}

/// `lua_register(L, name, f)`: install `f` as the global named `name`
/// (PUC `lua_register`, defined in lua.h as a macro over pushcfunction
/// + setglobal).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
/// `name` is null or a NUL-terminated string that stays valid for the call.
// SAFETY: no other item in the link is named `lua_register`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_register(L: *mut LuaState, name: *const c_char, f: LuaCFunction) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    unsafe { lua_pushcfunction(L, f) };
    // SAFETY: `L` as above, and `name` is null or NUL-terminated and valid for the call (# Safety)
    unsafe { lua_setglobal(L, name) };
}

/// Return the Lua version this state targets (e.g. 505 for 5.5), matching
/// PUC's `LUA_VERSION_NUM` shape.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_version`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_version(L: *mut LuaState) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    match vm.version() {
        LuaVersion::Lua51 => 501,
        LuaVersion::Lua52 => 502,
        LuaVersion::Lua53 => 503,
        LuaVersion::Lua54 => 504,
        // MacroLua reports the 5.4 base it inherits from.
        LuaVersion::MacroLua => 504,
        LuaVersion::Lua55 => 505,
    }
}
