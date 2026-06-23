//! C ABI surface — a minimal viable `lua.h`-equivalent that lets existing
//! C/C++ hosts link against luna. Not a full PUC reimplementation; the
//! subset here is the one needed to drive a Vm from a C caller:
//! create/close state, load + pcall, push/to integer/string/boolean/nil,
//! get/setglobal, stack height, type queries, plus C-side callbacks via
//! `lua_pushcfunction` / `lua_register`.
//!
//! `L` argument names follow PUC convention; suppress non_snake_case warnings
//! for the whole module so the surface reads like lua.h.
//!
//! Aliasing safety: `LuaState` is a `#[repr(transparent)]` wrapper around
//! `Vm`. A `*mut LuaState` is bit-identical to a `*mut Vm`, so any function
//! that holds `&mut Vm` can cast it to `*mut LuaState` and hand it to a C
//! callback without a second active reference colliding. The trampoline
//! that bridges to a `LuaCFunction` drops its `&mut Vm` to a raw pointer
//! exactly across the `cf(L)` call, re-borrowing afterward — see
//! `capi_trampoline` for the prose.
#![allow(non_snake_case)]

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Vm};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

/// `lua_State` analogue — a transparent wrapper around `Vm`. C callers see
/// it as an opaque pointer; Rust glue casts between `*mut LuaState` and
/// `*mut Vm` freely because `#[repr(transparent)]` guarantees identical
/// layout.
#[repr(transparent)]
pub struct LuaState {
    vm: Vm,
}

/// PUC status codes that fit luna's surface.
pub const LUA_OK: c_int = 0;
/// PUC `LUA_ERRRUN` — runtime error while executing a chunk.
pub const LUA_ERRRUN: c_int = 2;
/// PUC `LUA_ERRSYNTAX` — parse / compile error in `luaL_loadbufferx`.
pub const LUA_ERRSYNTAX: c_int = 3;
/// PUC `LUA_ERRMEM` — memory-allocation failure.
pub const LUA_ERRMEM: c_int = 4;

/// PUC `lua_type` constants — match the values PUC uses so a C header
/// shared with PUC code resolves to the same tags.
pub const LUA_TNONE: c_int = -1;
/// PUC `LUA_TNIL`.
pub const LUA_TNIL: c_int = 0;
/// PUC `LUA_TBOOLEAN`.
pub const LUA_TBOOLEAN: c_int = 1;
/// PUC `LUA_TLIGHTUSERDATA`.
pub const LUA_TLIGHTUSERDATA: c_int = 2;
/// PUC `LUA_TNUMBER`.
pub const LUA_TNUMBER: c_int = 3;
/// PUC `LUA_TSTRING`.
pub const LUA_TSTRING: c_int = 4;
/// PUC `LUA_TTABLE`.
pub const LUA_TTABLE: c_int = 5;
/// PUC `LUA_TFUNCTION`.
pub const LUA_TFUNCTION: c_int = 6;
/// PUC `LUA_TUSERDATA`.
pub const LUA_TUSERDATA: c_int = 7;
/// PUC `LUA_TTHREAD`.
pub const LUA_TTHREAD: c_int = 8;

/// C function ABI used by `lua_pushcfunction` / `lua_register`.
pub type LuaCFunction = extern "C" fn(*mut LuaState) -> c_int;

/// Resolve a (possibly negative) PUC-style index into a Vm `capi_stack`
/// slot. Returns None when the index is out of bounds.
fn abs_index(vm: &Vm, idx: c_int) -> Option<usize> {
    let len = vm.capi_stack.len() as c_int;
    let abs = if idx > 0 {
        idx
    } else if idx < 0 {
        len + idx + 1
    } else {
        return None; // 0 is invalid for PUC indices
    };
    if abs < 1 || abs > len {
        None
    } else {
        Some((abs - 1) as usize)
    }
}

fn get_at(vm: &Vm, idx: c_int) -> Option<Value> {
    abs_index(vm, idx).map(|i| vm.capi_stack[i])
}

unsafe fn vm_mut<'a>(L: *mut LuaState) -> &'a mut Vm {
    debug_assert!(!L.is_null(), "null lua_State*");
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    unsafe { &mut (*L).vm }
}

fn type_tag(v: Value) -> c_int {
    match v {
        Value::Nil => LUA_TNIL,
        Value::Bool(_) => LUA_TBOOLEAN,
        Value::Int(_) | Value::Float(_) => LUA_TNUMBER,
        Value::Str(_) => LUA_TSTRING,
        Value::Table(_) => LUA_TTABLE,
        Value::Closure(_) | Value::Native(_) => LUA_TFUNCTION,
        Value::Userdata(_) => LUA_TUSERDATA,
        Value::Coro(_) => LUA_TTHREAD,
        Value::LightUserdata(_) => LUA_TLIGHTUSERDATA,
    }
}

// ─── state lifecycle ─────────────────────────────────────────────────────

/// Allocate a new Lua state with the 5.5 dialect (PUC `luaL_newstate`).
/// The state is empty — call `luaL_openlibs` to load the standard library.
///
/// v1.1 A1 Session C — the C ABI is a `luna`-crate surface, so we
/// install the Cranelift backend here (matching v1.0 behavior where
/// JIT was on by default for C callers). luna-core's
/// `Vm::new_minimal` itself defaults to `NullJitBackend`.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub extern "C" fn luaL_newstate() -> *mut LuaState {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    vm.install_jit_backend(
        crate::jit_backend::CraneliftBackend,
        crate::jit_backend::CraneliftBackend,
    );
    let l = Box::new(LuaState { vm });
    Box::into_raw(l)
}

/// Free the state and its Vm (PUC `lua_close`). Safe to call with a null
/// pointer (no-op); calling with a previously-closed pointer is UB just
/// like in PUC.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_close(L: *mut LuaState) {
    if L.is_null() {
        return;
    }
    // SAFETY: `L` was originally produced by `Box::into_raw` in `lua_newstate` / `lua_open`; the caller hasn't freed it via another `lua_close`, so reclaiming ownership here is sound.
    let _ = unsafe { Box::from_raw(L) };
}

/// Open all 5.5 standard libraries (PUC `luaL_openlibs`).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_openlibs(L: *mut LuaState) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    vm.open_all_libs();
}

// ─── load + call ─────────────────────────────────────────────────────────

/// Compile `src` (NUL-terminated C string) under `chunkname`; push the
/// resulting function on the stack and return LUA_OK, or push the error
/// string and return LUA_ERRSYNTAX (PUC `luaL_loadstring`). `chunkname`
/// may be null — in that case the compiler uses `"=?"`.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_loadstring(L: *mut LuaState, src: *const c_char) -> c_int {
    if L.is_null() || src.is_null() {
        return LUA_ERRSYNTAX;
    }
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    // SAFETY: Lua C API contract — the caller guarantees the passed `*const c_char` points to a NUL-terminated byte string that stays valid for the duration of this call.
    let src_bytes = unsafe { CStr::from_ptr(src).to_bytes() };
    match vm.load(src_bytes, b"=(load)") {
        Ok(cl) => {
            vm.capi_stack.push(Value::Closure(cl));
            LUA_OK
        }
        Err(e) => {
            let msg = format!("{e}");
            let v = Value::Str(vm.heap.intern(msg.as_bytes()));
            vm.capi_stack.push(v);
            LUA_ERRSYNTAX
        }
    }
}

/// Call `stack[-(nargs + 1)]` with the top `nargs` values as arguments,
/// expecting `nresults` results (use -1 to mean "all"). Pops the function
/// + arguments and pushes the results; on error pushes the error message
/// and returns LUA_ERRRUN. `msgh` (message handler) is accepted for ABI
/// compatibility but currently ignored — the error object is forwarded
/// raw (PUC `lua_pcall` with `msgh=0` is the same).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
// SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
pub unsafe extern "C" fn lua_pcall(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    _msgh: c_int,
) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    let needed = (nargs + 1) as usize;
    if vm.capi_stack.len() < needed {
        let v = Value::Str(vm.heap.intern(b"not enough values on stack"));
        vm.capi_stack.push(v);
        return LUA_ERRRUN;
    }
    let func_idx = vm.capi_stack.len() - needed;
    let args: Vec<Value> = vm.capi_stack[func_idx + 1..].to_vec();
    let f = vm.capi_stack[func_idx];
    vm.capi_stack.truncate(func_idx);
    match vm.call_value(f, &args) {
        Ok(mut results) => {
            if nresults >= 0 {
                results.resize(nresults as usize, Value::Nil);
            }
            for v in results {
                vm.capi_stack.push(v);
            }
            LUA_OK
        }
        Err(e) => {
            let err_val = match e.0 {
                Value::Str(_) => e.0,
                _ => {
                    let rendered = vm.error_text(&e);
                    Value::Str(vm.heap.intern(rendered.as_bytes()))
                }
            };
            vm.capi_stack.push(err_val);
            LUA_ERRRUN
        }
    }
}

// ─── globals ─────────────────────────────────────────────────────────────

/// Push the global named by `name` on the stack and return its type
/// (`LUA_T*`). `LUA_TNIL` if the global is unset (PUC `lua_getglobal`).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getglobal(L: *mut LuaState, name: *const c_char) -> c_int {
    if name.is_null() {
        return LUA_TNONE;
    }
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    // SAFETY: Lua C API contract — the caller guarantees the passed `*const c_char` points to a NUL-terminated byte string that stays valid for the duration of this call.
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let key = Value::Str(vm.heap.intern(name_bytes));
    let v = vm.globals().get(key);
    vm.capi_stack.push(v);
    type_tag(v)
}

/// Pop the top of the stack and set it as the global named by `name`
/// (PUC `lua_setglobal`).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setglobal(L: *mut LuaState, name: *const c_char) {
    if name.is_null() {
        return;
    }
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    let v = vm.capi_stack.pop().unwrap_or(Value::Nil);
    // SAFETY: Lua C API contract — the caller guarantees the passed `*const c_char` points to a NUL-terminated byte string that stays valid for the duration of this call.
    let name_str = unsafe { CStr::from_ptr(name).to_str().unwrap_or("?") };
    let _ = vm.set_global(name_str, v);  // capi swallows: lua_setglobal is void in C ABI
}

// ─── stack push ──────────────────────────────────────────────────────────

/// PUC `lua_pushnil` — push `nil` onto the C API stack.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushnil(L: *mut LuaState) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.push(Value::Nil);
}

/// PUC `lua_pushboolean` — push a boolean (`0` is false, anything else true).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushboolean(L: *mut LuaState, b: c_int) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.push(Value::Bool(b != 0));
}

/// PUC `lua_pushinteger` — push a 64-bit signed integer.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushinteger(L: *mut LuaState, n: i64) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.push(Value::Int(n));
}

/// PUC `lua_pushnumber` — push an IEEE-754 double.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushnumber(L: *mut LuaState, n: f64) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.push(Value::Float(n));
}

/// Push `str` (NUL-terminated) on the stack and return a borrowed pointer
/// to its interned bytes. Lifetime: until the pushed string falls off the
/// stack (or the state is closed).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushstring(L: *mut LuaState, str: *const c_char) -> *const c_char {
    if str.is_null() {
        // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
        unsafe { lua_pushnil(L) };
        return std::ptr::null();
    }
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    // SAFETY: Lua C API contract — the caller guarantees the passed `*const c_char` points to a NUL-terminated byte string that stays valid for the duration of this call.
    let bytes = unsafe { CStr::from_ptr(str).to_bytes() };
    let interned = vm.heap.intern(bytes);
    vm.capi_stack.push(Value::Str(interned));
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    unsafe { lua_tostring(L, -1) }
}

// ─── stack read ──────────────────────────────────────────────────────────

/// PUC `lua_tointeger` — convert the value at `idx` to `i64` (PUC's lossy
/// coercion: floats truncate, booleans become 0/1, parsable strings parse,
/// others return 0).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tointeger(L: *mut LuaState, idx: c_int) -> i64 {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
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
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tonumber(L: *mut LuaState, idx: c_int) -> f64 {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
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
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_toboolean(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
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
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_tostring(L: *mut LuaState, idx: c_int) -> *const c_char {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
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

// ─── type queries ────────────────────────────────────────────────────────

/// PUC `lua_type` — discriminator tag at `idx` (`LUA_T*`); `LUA_TNONE`
/// if `idx` is out of bounds.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_type(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    get_at(vm, idx).map_or(LUA_TNONE, type_tag)
}

/// PUC `lua_isnil` — true iff the value at `idx` is `nil`.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isnil(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    (unsafe { lua_type(L, idx) } == LUA_TNIL) as c_int
}

/// PUC `lua_isnumber` — true iff the value at `idx` is `Int` or `Float`.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isnumber(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    matches!(
        get_at(vm, idx),
        Some(Value::Int(_)) | Some(Value::Float(_))
    ) as c_int
}

/// PUC `lua_isinteger` (5.3+) — true iff the value at `idx` is exactly `Int`.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isinteger(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    matches!(get_at(vm, idx), Some(Value::Int(_))) as c_int
}

/// PUC `lua_isstring` — true iff the value at `idx` is a string or a
/// number (numbers coerce to strings in PUC).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isstring(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    matches!(
        get_at(vm, idx),
        Some(Value::Str(_)) | Some(Value::Int(_)) | Some(Value::Float(_))
    ) as c_int
}

/// PUC `lua_isboolean` — true iff the value at `idx` is a boolean.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isboolean(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    matches!(get_at(vm, idx), Some(Value::Bool(_))) as c_int
}

/// PUC `lua_isfunction` — true iff the value at `idx` is a Lua closure
/// or a native function.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_isfunction(L: *mut LuaState, idx: c_int) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    matches!(
        get_at(vm, idx),
        Some(Value::Closure(_)) | Some(Value::Native(_))
    ) as c_int
}

// ─── stack manipulation ──────────────────────────────────────────────────

/// PUC `lua_gettop` — current stack height (number of pushed values).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_gettop(L: *mut LuaState) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    vm.capi_stack.len() as c_int
}

/// PUC `lua_settop` — set the stack height to `idx`, padding with `nil`
/// or truncating as needed (negative indices count from the top).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_settop(L: *mut LuaState, idx: c_int) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    let new_len = if idx >= 0 {
        idx as usize
    } else {
        (vm.capi_stack.len() as c_int + idx + 1).max(0) as usize
    };
    if new_len < vm.capi_stack.len() {
        vm.capi_stack.truncate(new_len);
    } else {
        vm.capi_stack.resize(new_len, Value::Nil);
    }
}

/// PUC `lua_pop` — drop the top `n` stack values.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pop(L: *mut LuaState, n: c_int) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    unsafe { lua_settop(L, -n - 1) };
}

/// PUC `lua_pushvalue` — duplicate the value at `idx` onto the top.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushvalue(L: *mut LuaState, idx: c_int) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    if let Some(v) = get_at(vm, idx) {
        vm.capi_stack.push(v);
    } else {
        vm.capi_stack.push(Value::Nil);
    }
}

// ─── C callbacks ─────────────────────────────────────────────────────────

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
        Value::LightUserdata(p) => unsafe {
            std::mem::transmute::<*const (), LuaCFunction>(p)
        },
        _ => {
            let s = Value::Str(vm.heap.intern(b"missing C function pointer upvalue"));
            return Err(LuaError(s));
        }
    };
    // Mirror args from the Vm dispatch frame to the C-visible capi_stack
    // (via the public `nat_arg` accessor — works for the missing-arg-is-nil
    // contract too).
    let baseline = vm.capi_stack.len();
    for i in 0..nargs {
        let v = vm.nat_arg(fs, nargs, i);
        vm.capi_stack.push(v);
    }
    // Demote to raw pointer; the &mut Vm is no longer live across the cf call.
    let vm_ptr: *mut Vm = vm as *mut Vm;
    let nret = cf(vm_ptr as *mut LuaState) as usize;
    // Re-borrow.
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { &mut *vm_ptr };
    let stack_len = vm.capi_stack.len();
    if stack_len < baseline + nret {
        // C function lied about its return count.
        let s = Value::Str(vm.heap.intern(b"C function returned more values than were pushed"));
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
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pushcfunction(L: *mut LuaState, f: LuaCFunction) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    // SAFETY: source and destination types share the same in-memory representation; see the C ABI typedef this function implements.
    let cf_ptr = unsafe { std::mem::transmute::<LuaCFunction, *const ()>(f) };
    let trampoline: luna_core::runtime::value::NativeFn = capi_trampoline;
    let f_val = vm.native_with(
        trampoline,
        Box::new([Value::LightUserdata(cf_ptr)]),
    );
    vm.capi_stack.push(f_val);
}

/// `lua_register(L, name, f)`: install `f` as the global named `name`
/// (PUC `lua_register`, defined in lua.h as a macro over pushcfunction
/// + setglobal).
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
// SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
pub unsafe extern "C" fn lua_register(
    L: *mut LuaState,
    name: *const c_char,
    f: LuaCFunction,
) {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    unsafe { lua_pushcfunction(L, f) };
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    unsafe { lua_setglobal(L, name) };
}

// ─── version probe ───────────────────────────────────────────────────────

/// Return the Lua version this state targets (e.g. 505 for 5.5), matching
/// PUC's `LUA_VERSION_NUM` shape.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_version(L: *mut LuaState) -> c_int {
    // SAFETY: Lua C API contract — the caller guarantees `L` is a valid `lua_State` pointer that this thread currently owns; pointer/index arguments follow the documented Lua API requirements.
    let vm = unsafe { vm_mut(L) };
    match vm.version() {
        LuaVersion::Lua51 => 501,
        LuaVersion::Lua52 => 502,
        LuaVersion::Lua53 => 503,
        LuaVersion::Lua54 => 504,
        LuaVersion::Lua55 => 505,
    }
}
