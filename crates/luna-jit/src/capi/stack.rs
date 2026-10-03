//! Stack push, read, type-query and manipulation entry points.

use super::*;

/// PUC `lua_pushnil` — push `nil` onto the C API stack.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pushnil`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushnil(L: *mut LuaState) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.push(Value::Nil);
}

/// PUC `lua_pushboolean` — push a boolean (`0` is false, anything else true).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pushboolean`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushboolean(L: *mut LuaState, b: c_int) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.push(Value::Bool(b != 0));
}

/// PUC `lua_pushinteger` — push a 64-bit signed integer.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pushinteger`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushinteger(L: *mut LuaState, n: i64) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.push(Value::Int(n));
}

/// PUC `lua_pushnumber` — push an IEEE-754 double.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pushnumber`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushnumber(L: *mut LuaState, n: f64) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.push(Value::Float(n));
}

/// Push `str` (NUL-terminated) on the stack and return a borrowed pointer
/// to its interned bytes. Lifetime: until the pushed string falls off the
/// stack (or the state is closed).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
/// `str` is null or a NUL-terminated string that stays valid for the call.
// SAFETY: no other item in the link is named `lua_pushstring`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushstring(L: *mut LuaState, str: *const c_char) -> *const c_char {
    if str.is_null() {
        // SAFETY: `L` is an open state no other call is using (# Safety)
        unsafe { lua_pushnil(L) };
        return std::ptr::null();
    }
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    // SAFETY: `str` is non-null (checked above), NUL-terminated and valid for this call (# Safety)
    let bytes = unsafe { CStr::from_ptr(str).to_bytes() };
    let interned = vm.heap.intern(bytes);
    vm.capi_stack.push(Value::Str(interned));
    // SAFETY: `L` as above, and `vm` is not used again
    unsafe { lua_tostring(L, -1) }
}

/// PUC `lua_tointeger` — convert the value at `idx` to `i64` (PUC's lossy
/// coercion: floats truncate, booleans become 0/1, parsable strings parse,
/// others return 0).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_tointeger`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tointeger(L: *mut LuaState, idx: c_int) -> i64 {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    match get_at(vm, idx) {
        Some(Value::Int(i)) => i,
        Some(Value::Float(f)) => f as i64,
        Some(Value::Bool(true)) => 1,
        Some(Value::Bool(false)) => 0,
        Some(Value::Str(st)) => std::str::from_utf8(st.as_bytes())
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

/// PUC `lua_tonumber` — convert the value at `idx` to `f64` with PUC's
/// lossy coercion (see [`lua_tointeger`]).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_tonumber`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tonumber(L: *mut LuaState, idx: c_int) -> f64 {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    match get_at(vm, idx) {
        Some(Value::Int(i)) => i as f64,
        Some(Value::Float(f)) => f,
        Some(Value::Bool(true)) => 1.0,
        Some(Value::Bool(false)) => 0.0,
        Some(Value::Str(st)) => std::str::from_utf8(st.as_bytes())
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

/// PUC `lua_toboolean` — Lua truth at `idx` (`nil` / `false` → 0; else 1).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_toboolean`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_toboolean(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    match get_at(vm, idx) {
        Some(Value::Nil) | None => 0,
        Some(Value::Bool(false)) => 0,
        _ => 1,
    }
}

/// Return a pointer to the i-th stack slot as a NUL-terminated string.
/// Numeric values are stringified the same way `tostring()` would. The
/// pointer is valid until the next `lua_tostring` on this state with a
/// different value, or `lua_close`.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_tostring`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tostring(L: *mut LuaState, idx: c_int) -> *const c_char {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    let bytes: Vec<u8> = match get_at(vm, idx) {
        Some(Value::Str(st)) => st.as_bytes().to_vec(),
        Some(Value::Int(i)) => i.to_string().into_bytes(),
        Some(Value::Float(f)) => f.to_string().into_bytes(),
        Some(Value::Nil) | None => return std::ptr::null(),
        Some(Value::Bool(true)) => b"true".to_vec(),
        Some(Value::Bool(false)) => b"false".to_vec(),
        _ => return std::ptr::null(),
    };
    let c = CString::new(bytes).unwrap_or_else(|_| CString::new("?").unwrap());
    let p = c.as_ptr();
    vm.capi_cstr_pin = Some(c);
    p
}

/// PUC `lua_type` — discriminator tag at `idx` (`LUA_T*`); `LUA_TNONE`
/// if `idx` is out of bounds.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_type`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_type(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    get_at(vm, idx).map_or(LUA_TNONE, type_tag)
}

/// PUC `lua_isnil` — true iff the value at `idx` is `nil`.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_isnil`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isnil(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    (unsafe { lua_type(L, idx) } == LUA_TNIL) as c_int
}

/// PUC `lua_isnumber` — true iff the value at `idx` is `Int` or `Float`.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_isnumber`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isnumber(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    matches!(get_at(vm, idx), Some(Value::Int(_)) | Some(Value::Float(_))) as c_int
}

/// PUC `lua_isinteger` (5.3+) — true iff the value at `idx` is exactly `Int`.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_isinteger`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isinteger(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    matches!(get_at(vm, idx), Some(Value::Int(_))) as c_int
}

/// PUC `lua_isstring` — true iff the value at `idx` is a string or a
/// number (numbers coerce to strings in PUC).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_isstring`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isstring(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    matches!(
        get_at(vm, idx),
        Some(Value::Str(_)) | Some(Value::Int(_)) | Some(Value::Float(_))
    ) as c_int
}

/// PUC `lua_isboolean` — true iff the value at `idx` is a boolean.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_isboolean`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isboolean(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    matches!(get_at(vm, idx), Some(Value::Bool(_))) as c_int
}

/// PUC `lua_isfunction` — true iff the value at `idx` is a Lua closure
/// or a native function.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_isfunction`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isfunction(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    matches!(
        get_at(vm, idx),
        Some(Value::Closure(_)) | Some(Value::Native(_))
    ) as c_int
}

/// PUC `lua_gettop` — current stack height (number of pushed values).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_gettop`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gettop(L: *mut LuaState) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    (vm.capi_stack.len() - vm.capi_base) as c_int
}

/// PUC `lua_settop` — set the stack height to `idx`, padding with `nil`
/// or truncating as needed (negative indices count from the top).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_settop`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_settop(L: *mut LuaState, idx: c_int) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    let base = vm.capi_base;
    let new_len = if idx >= 0 {
        base + idx as usize
    } else {
        base + ((vm.capi_stack.len() - base) as c_int + idx + 1).max(0) as usize
    };
    if new_len < vm.capi_stack.len() {
        vm.capi_stack.truncate(new_len);
    } else {
        vm.capi_stack.resize(new_len, Value::Nil);
    }
}

/// PUC `lua_pop` — drop the top `n` stack values.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pop`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pop(L: *mut LuaState, n: c_int) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    unsafe { lua_settop(L, -n - 1) };
}

/// PUC `lua_pushvalue` — duplicate the value at `idx` onto the top.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pushvalue`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushvalue(L: *mut LuaState, idx: c_int) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    if let Some(v) = get_at(vm, idx) {
        vm.capi_stack.push(v);
    } else {
        vm.capi_stack.push(Value::Nil);
    }
}
