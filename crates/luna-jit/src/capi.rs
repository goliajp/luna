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
/// PUC `LUA_ERRERR` — the message handler of `lua_pcall` itself failed.
/// This is the value in 5.1, 5.4 and 5.5; a 5.2 or 5.3 state returns 6,
/// that dialect's `LUA_ERRERR`.
pub const LUA_ERRERR: c_int = 5;

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
    let base = vm.capi_base;
    let len = (vm.capi_stack.len() - base) as c_int;
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
        Some(base + (abs - 1) as usize)
    }
}

fn get_at(vm: &Vm, idx: c_int) -> Option<Value> {
    abs_index(vm, idx).map(|i| vm.capi_stack[i])
}

/// PUC `lua_error` for an error a C API function raised. Inside a C
/// function luna called, the error is thrown when that function returns,
/// in place of its results, so the protected call around it receives it.
/// Outside one nothing can catch it: PUC's default panic function prints
/// it and the process aborts.
fn raise(vm: &mut Vm, e: LuaError) {
    if vm.capi_calls == 0 {
        let msg = match e.0 {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            v => format!("error object is a {} value", v.type_name()),
        };
        eprintln!("PANIC: unprotected error in call to Lua API ({msg})");
        std::process::abort();
    }
    // the first error ends the C function in PUC; later calls cannot
    // replace it
    if vm.capi_error.is_none() {
        vm.capi_error = Some(e.0);
    }
}

/// The Vm behind `L`.
///
/// # Safety
/// `L` points at a live `LuaState`, and no other reference to its Vm is
/// used while the returned one is.
unsafe fn vm_mut<'a>(L: *mut LuaState) -> &'a mut Vm {
    debug_assert!(!L.is_null(), "null lua_State*");
    // SAFETY: the caller's contract
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
// SAFETY: no other item in the link is named `luaL_newstate`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub extern "C" fn luaL_newstate() -> *mut LuaState {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    crate::install_default_jit(&mut vm);
    let l = Box::new(LuaState { vm });
    Box::into_raw(l)
}

/// Allocate a new Lua state for the dialect whose `LUA_VERSION_NUM` is
/// `version` (501 to 505), otherwise like `luaL_newstate`. Returns null for
/// any other number.
// SAFETY: no other item in the link is named `luna_newstate`: PUC's liblua
// has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub extern "C" fn luna_newstate(version: c_int) -> *mut LuaState {
    let v = match version {
        501 => LuaVersion::Lua51,
        502 => LuaVersion::Lua52,
        503 => LuaVersion::Lua53,
        504 => LuaVersion::Lua54,
        505 => LuaVersion::Lua55,
        _ => return std::ptr::null_mut(),
    };
    let mut vm = Vm::new_minimal(v);
    crate::install_default_jit(&mut vm);
    Box::into_raw(Box::new(LuaState { vm }))
}

/// Free the state and its Vm (PUC `lua_close`). Safe to call with a null
/// pointer (no-op); calling with a previously-closed pointer is UB just
/// like in PUC.
///
/// # Safety
/// `L` is null or a state from `luaL_newstate` that has not been closed, and no call on it
/// is running.
// SAFETY: no other item in the link is named `lua_close`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_close(L: *mut LuaState) {
    if L.is_null() {
        return;
    }
    // SAFETY: `L` is non-null, so it is the box `luaL_newstate` leaked, not closed before and not
    // in use (# Safety); this takes ownership back once
    let _ = unsafe { Box::from_raw(L) };
}

/// Open all 5.5 standard libraries (PUC `luaL_openlibs`).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `luaL_openlibs`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_openlibs(L: *mut LuaState) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    vm.open_all_libs();
}

// ─── load + call ─────────────────────────────────────────────────────────

/// Compile `src` (NUL-terminated C string) under `chunkname`; push the
/// resulting function on the stack and return LUA_OK, or push the error
/// string and return LUA_ERRSYNTAX (PUC `luaL_loadstring`). As in PUC,
/// the source itself is the chunk name, so messages name the chunk
/// `[string "..."]`.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
/// `src` is null or a NUL-terminated string that stays valid for the call.
// SAFETY: no other item in the link is named `luaL_loadstring`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaL_loadstring(L: *mut LuaState, src: *const c_char) -> c_int {
    if L.is_null() || src.is_null() {
        return LUA_ERRSYNTAX;
    }
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    // SAFETY: `src` is non-null (checked above), NUL-terminated and valid for this call (# Safety)
    let src_bytes = unsafe { CStr::from_ptr(src).to_bytes() };
    match vm.load(src_bytes, src_bytes) {
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
/// and arguments and pushes the results (PUC `lua_pcall`).
///
/// On an error it pushes the error object and returns LUA_ERRRUN. A
/// nonzero `msgh` is the stack index of a message handler: it runs where
/// the error was raised, before the stack unwinds, and its first result
/// becomes the error object. When the handler itself fails the object is
/// "error in error handling" and the status is the dialect's LUA_ERRERR
/// (5 in 5.1, 5.4 and 5.5; 6 in 5.2 and 5.3).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_pcall`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_pcall(
    L: *mut LuaState,
    nargs: c_int,
    nresults: c_int,
    msgh: c_int,
) -> c_int {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    let needed = (nargs + 1) as usize;
    if vm.capi_stack.len() - vm.capi_base < needed {
        let v = Value::Str(vm.heap.intern(b"not enough values on stack"));
        vm.capi_stack.push(v);
        return LUA_ERRRUN;
    }
    // the index is resolved before the function and arguments are popped
    let handler = (msgh != 0).then(|| get_at(vm, msgh).unwrap_or(Value::Nil));
    let func_idx = vm.capi_stack.len() - needed;
    let args: Vec<Value> = vm.capi_stack[func_idx + 1..].to_vec();
    let f = vm.capi_stack[func_idx];
    vm.capi_stack.truncate(func_idx);
    let r = match handler {
        Some(h) => vm.call_value_with_handler_status(f, &args, h),
        None => vm.call_value(f, &args).map_err(|e| (e, false)),
    };
    match r {
        Ok(mut results) => {
            if nresults >= 0 {
                results.resize(nresults as usize, Value::Nil);
            }
            vm.capi_stack.extend(results);
            LUA_OK
        }
        Err((e, errerr)) => {
            // 5.5's `luaG_errormsg` names a nil error object; with a
            // handler the vm has already done so
            let obj = if e.0.is_nil() && vm.version() >= LuaVersion::Lua55 {
                Value::Str(vm.heap.intern(b"<no error object>"))
            } else {
                e.0
            };
            vm.capi_stack.push(obj);
            match (errerr, vm.version()) {
                (false, _) => LUA_ERRRUN,
                (true, LuaVersion::Lua52 | LuaVersion::Lua53) => 6,
                (true, _) => LUA_ERRERR,
            }
        }
    }
}

// ─── globals ─────────────────────────────────────────────────────────────

/// Push the global named by `name` on the stack and return its type
/// (`LUA_T*`). `LUA_TNIL` if the global is unset (PUC `lua_getglobal`).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
/// `name` is null or a NUL-terminated string that stays valid for the call.
// SAFETY: no other item in the link is named `lua_getglobal`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getglobal(L: *mut LuaState, name: *const c_char) -> c_int {
    if name.is_null() {
        return LUA_TNONE;
    }
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    // SAFETY: `name` is non-null (checked above), NUL-terminated and valid for this call (# Safety)
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let key = Value::Str(vm.heap.intern(name_bytes));
    let g = Value::Table(vm.globals());
    // `_G`'s `__index` runs, and an error in it is raised
    let v = match vm.index_with_mm(g, key) {
        Ok(v) => v,
        Err(e) => {
            raise(vm, e);
            Value::Nil
        }
    };
    vm.capi_stack.push(v);
    type_tag(v)
}

/// Pop the top of the stack and set it as the global named by `name`
/// (PUC `lua_setglobal`).
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
/// `name` is null or a NUL-terminated string that stays valid for the call.
// SAFETY: no other item in the link is named `lua_setglobal`: the host does not link PUC's liblua
// next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setglobal(L: *mut LuaState, name: *const c_char) {
    if name.is_null() {
        return;
    }
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    let v = if vm.capi_stack.len() > vm.capi_base {
        vm.capi_stack.pop().unwrap_or(Value::Nil)
    } else {
        Value::Nil
    };
    // SAFETY: `name` is non-null (checked above), NUL-terminated and valid for this call (# Safety)
    let name_bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    let key = Value::Str(vm.heap.intern(name_bytes));
    let g = Value::Table(vm.globals());
    // `_G`'s `__newindex` runs; its error, or a read-only `_G`'s, is raised
    if let Err(e) = vm.set_index_with_mm(g, key, v) {
        raise(vm, e);
    }
}

/// Mark the table at `idx` read-only (`enabled` nonzero) or writable
/// again (Redis's `lua_enablereadonlytable`): see `Vm::set_readonly`. A
/// value that is not a table is left alone.
///
/// # Safety
/// `L` is a state from `luaL_newstate` that `lua_close` has not freed, and no other API
/// call on it is running other than a C function it is calling into.
// SAFETY: no other item in the link is named `lua_enablereadonlytable`: the host does not
// link PUC's or Redis's liblua next to this crate, which defines each `lua_*` symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_enablereadonlytable(L: *mut LuaState, idx: c_int, enabled: c_int) {
    // SAFETY: `L` is an open state no other call is using (# Safety)
    let vm = unsafe { vm_mut(L) };
    if let Some(Value::Table(t)) = get_at(vm, idx) {
        vm.set_readonly(t, enabled != 0);
    }
}

mod callbacks;
mod stack;
pub use callbacks::*;
pub use stack::*;
