//! The C API: PUC Lua's `lua.h`, `lauxlib.h` and `lualib.h` for C and C++
//! hosts, for each of the five dialects. The headers are in `include/`, one
//! directory per dialect (`lua5.1` to `lua5.5`); a host compiled against one
//! of them gets a state of that dialect from `luaL_newstate`, and every
//! function behaves as that version of PUC Lua does.
//!
//! A `lua_State` is one thread: the main thread or a coroutine. Each has
//! its own stack of values as C sees it, kept on the thread
//! (`Coro::host_stack`), and its own `LuaState` record, made the first time
//! C asks for the thread.
//!
//! Errors leave a C function at once, as in PUC: the API functions that can
//! raise an error or yield are C functions (`csrc/`), which call the Rust
//! side, and when it reports an error, `longjmp` to the boundary set up just
//! before luna called the C function. The Rust side has always returned by
//! then, so the jump crosses only C frames. The exported names of those
//! functions are jumps to the C functions (`exports.rs`), because a shared
//! library built by Rust exports only symbols defined in Rust.
//!
//! `L` argument names follow PUC; the module allows non_snake_case so the
//! surface reads like `lua.h`.
#![allow(non_snake_case)]

use luna_core::runtime::mem::LVec;
use luna_core::runtime::{Coro, Gc, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Vm};
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int};

#[macro_use]
mod exports;

mod access;
mod access2;
mod api;
mod auxlib;
mod callbacks;
mod calls;
mod ccall;
mod debug;
mod gc;
mod hooks;
mod libs;
mod load;
mod meta;
mod ops;
mod push;
mod stack;
mod state;
mod tables;
mod tables_set;
mod tbc;
mod threads;
#[cfg(test)]
mod unit_tests;
mod userdata;

pub use access::*;
pub use access2::*;
pub use callbacks::*;
pub use calls::*;
pub use ccall::LuaHook;
pub use push::*;
pub use stack::*;
pub use state::*;

use api::Api;

/// PUC status codes.
pub const LUA_OK: c_int = 0;
/// PUC `LUA_YIELD` — a coroutine is suspended.
pub const LUA_YIELD: c_int = 1;
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

/// C function ABI (PUC `lua_CFunction`).
pub type LuaCFunction = extern "C" fn(*mut LuaState) -> c_int;
/// 5.3+ continuation function (PUC `lua_KFunction`).
pub type LuaKFunction = extern "C" fn(*mut LuaState, c_int, isize) -> c_int;

/// PUC's `LUA_T*` tag of a value.
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

/// `LUA_VERSION_NUM` of a dialect. MacroLua reports the 5.4 base it
/// inherits from.
fn version_num(v: LuaVersion) -> c_int {
    match v {
        LuaVersion::Lua51 => 501,
        LuaVersion::Lua52 => 502,
        LuaVersion::Lua53 => 503,
        LuaVersion::Lua54 | LuaVersion::MacroLua => 504,
        LuaVersion::Lua55 => 505,
    }
}

/// The bytes of a C string, or `None` for a null pointer.
///
/// # Safety
/// `s` is null or a NUL-terminated string valid for `'a`.
unsafe fn c_bytes<'a>(s: *const c_char) -> Option<&'a [u8]> {
    // SAFETY: the caller's contract
    (!s.is_null()).then(|| unsafe { CStr::from_ptr(s) }.to_bytes())
}
