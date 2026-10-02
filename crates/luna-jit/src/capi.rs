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
/// The C ABI is a `luna`-crate surface, so the JIT is on by default
/// for C callers. luna-core's `Vm::new_minimal` itself defaults to
/// `NullJitBackend`.
///
/// Route through `crate::install_default_jit` so the `CraneliftBackend` install is paired with a fresh
/// `CraneliftJitStorage`. Calling `install_jit_backend` alone leaves
/// `Vm.jit.storage` at the default `NullJitStorage`; the trait-pair
/// invariant means the first JIT compile path that downcasts storage
/// (via `jit_backend::storage::from_storage`) would observe a type
/// mismatch. `storage::from_storage` returns a `Result`, so a mismatch
/// skips JIT instead of panicking across the C-ABI boundary, but the
/// right thing here is still to install both halves so the JIT actually runs for capi callers.
// SAFETY: `no_mangle` is required for the C ABI symbol to be linkable as `lua_*` by external C/C++ callers; this crate is the sole producer of these symbols within any final binary that links it.
#[unsafe(no_mangle)]
pub extern "C" fn luaL_newstate() -> *mut LuaState {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    crate::install_default_jit(&mut vm);
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
    let _ = vm.set_global(name_str, v); // capi swallows: lua_setglobal is void in C ABI
}

mod callbacks;
mod stack;
pub use callbacks::*;
pub use stack::*;
