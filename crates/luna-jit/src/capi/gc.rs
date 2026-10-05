//! The collector's controls (`lua_gc`), warnings, the allocation
//! function, the panic function and what it sees.

use super::ccall::{take_error, with_c};
use super::*;

/// PUC 5.4+ `lua_WarnFunction`.
pub type LuaWarnFunction = unsafe extern "C" fn(ud: *mut c_void, msg: *const c_char, tocont: c_int);

// SAFETY: the declaration matches the definition in `csrc/shim_load.c`;
// it calls the warning function under a fresh error boundary and returns
// the status it threw. C sees `lua_State` as opaque and reads only its
// leading fields, which `csrc/shim.h` declares
#[allow(improper_ctypes)]
unsafe extern "C" {
    fn luna_c_protect_warn(
        L: *mut LuaState,
        f: LuaWarnFunction,
        ud: *mut c_void,
        msg: *const c_char,
        tocont: c_int,
    ) -> c_int;
}

c_exports! {
    lua_gc => luna_c_lua_gc,
    luna_gc_51 => luna_c_luna_gc_51,
    lua_warning => luna_c_lua_warning,
}

fn gc(api: &mut Api, what: c_int, args: [i64; 3]) -> c_int {
    match api.vm.host_gc(what, args) {
        Ok(r) => r,
        Err(e) => {
            api.raise(e);
            0
        }
    }
}

/// The Rust side of 5.4/5.5 `lua_gc` (`csrc/shim_load.c` reads the
/// variadic arguments of the option).
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// called by the C function `lua_gc`, which throws what this raises.
// SAFETY: no other item in the link is named `luna_capi_gc`; the C function
// `luna_c_lua_gc` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_gc(
    L: *mut LuaState,
    what: c_int,
    a: i64,
    b: i64,
    c: i64,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    gc(&mut api, what, [a, b, c])
}

/// PUC 5.1 to 5.3 `lua_gc(L, what, data)`.
///
/// # Safety
/// As [`luna_capi_gc`]; called by the C wrapper.
// SAFETY: no other item in the link is named `luna_capi_luna_gc_51`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_luna_gc_51(L: *mut LuaState, what: c_int, data: c_int) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    gc(&mut api, what, [i64::from(data), 0, 0])
}

/// One piece of a warning to the host's warning function `f`, called on
/// the state's main thread under a boundary: an error it raises comes
/// back as the `Err`.
fn call_warnf(
    vm: &mut Vm,
    main: *mut LuaState,
    f: LuaWarnFunction,
    ud: *mut c_void,
    msg: &[u8],
    tocont: bool,
) -> Result<(), LuaError> {
    let mut text = Vec::with_capacity(msg.len() + 1);
    text.extend_from_slice(msg);
    text.push(0);
    // SAFETY: `main` is the live main thread of `vm`'s state, `f` the
    // host's warning function and `text` NUL-terminated
    let status = with_c(vm, main, || unsafe {
        luna_c_protect_warn(main, f, ud, text.as_ptr().cast(), c_int::from(tocont))
    });
    if status == LUA_OK {
        Ok(())
    } else {
        Err(LuaError(take_error(main)))
    }
}

/// PUC `lua_setwarnf`: `f` receives every piece of a warning from now on,
/// with `ud`; a null `f` turns warnings off.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into; `f` is null or a
/// warning function that may be called with `ud` until the state closes or
/// another is set.
// SAFETY: no other item in the link is named `lua_setwarnf`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setwarnf(
    L: *mut LuaState,
    f: Option<LuaWarnFunction>,
    ud: *mut c_void,
) {
    // SAFETY: the caller's contract (# Safety)
    let api = unsafe { Api::new(L) };
    let main = state_of(api.vm, api.vm.host_main_thread());
    let w: luna_core::vm::exec::host_c::HostWarn = match f {
        Some(f) => Box::new(move |vm, msg, tocont| call_warnf(vm, main, f, ud, msg, tocont)),
        None => Box::new(|_, _, _| Ok(())),
    };
    api.vm.set_host_warn(Some(w));
}

/// PUC `lua_warning`: one piece of a warning, `tocont` when more follow.
///
/// # Safety
/// `L` is a live thread of an open state, the innermost API call on it;
/// `msg` is NUL-terminated; called by the C wrapper, which throws what
/// this raises.
// SAFETY: no other item in the link is named `luna_capi_lua_warning`; the C
// wrapper is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_lua_warning(
    L: *mut LuaState,
    msg: *const c_char,
    tocont: c_int,
) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    // SAFETY: `msg` is NUL-terminated (# Safety)
    let text = unsafe { c_bytes(msg) }.unwrap_or_default();
    if let Err(e) = api.vm.host_warning(text, tocont != 0) {
        api.raise(e);
    }
}

/// PUC `lua_atpanic`: set the panic function and return the previous one.
///
/// # Safety
/// `L` is a live thread of an open state.
// SAFETY: no other item in the link is named `lua_atpanic`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_atpanic(
    L: *mut LuaState,
    panicf: Option<LuaCFunction>,
) -> Option<LuaCFunction> {
    // SAFETY: `L` is live (# Safety), so is its global record; one field is
    // swapped
    unsafe { std::mem::replace(&mut (*(*L).g).panic, panicf) }
}

/// PUC `lua_getallocf`: the allocation function and, through `ud` when it
/// is not null, its user data.
///
/// # Safety
/// `L` is a live thread of an open state; `ud` is null or writable.
// SAFETY: no other item in the link is named `lua_getallocf`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getallocf(L: *mut LuaState, ud: *mut *mut c_void) -> Option<LuaAlloc> {
    // SAFETY: `L` is live (# Safety), so is its global record; `ud` is
    // null or writable
    unsafe {
        let g = (*L).g;
        if !ud.is_null() {
            *ud = (*g).alloc_ud;
        }
        (*g).alloc
    }
}

/// PUC `lua_setallocf`: every block of the state is resized and freed
/// through `f` from now on, and new blocks come from it.
///
/// # Safety
/// `L` is a live thread of an open state; `f` can free what the previous
/// function allocated, as PUC requires.
// SAFETY: no other item in the link is named `lua_setallocf`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setallocf(L: *mut LuaState, f: Option<LuaAlloc>, ud: *mut c_void) {
    // SAFETY: `L` is live (# Safety), so are its global record and Vm; `f`
    // accepts the blocks of the previous function (# Safety)
    unsafe {
        let g = (*L).g;
        (*g).alloc = f;
        (*g).alloc_ud = ud;
        if let Some(f) = f {
            (*(*g).vm).heap.mem_ctx().set_raw_alloc(f, ud);
        }
    }
}

/// PUC 5.4 `lua_setcstacklimit`, which since 5.4.3 changes nothing and
/// returns `LUAI_MAXCCALLS`.
// SAFETY: no other item in the link is named `lua_setcstacklimit`: the host
// does not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub extern "C" fn lua_setcstacklimit(_L: *mut LuaState, _limit: std::os::raw::c_uint) -> c_int {
    200
}

/// The stack of `L` as PUC's panic function sees it, the error object on
/// top: 5.1 resets the thread to its base level with the error alone on
/// it; 5.3 pushes a copy of the error; 5.4 and 5.5 close the thread's
/// to-be-closed slots with the error (an error in one replacing it) and
/// leave the error alone on the stack.
///
/// # Safety
/// `L` is a live thread of an open state with no API call running on it
/// but the one throwing.
// SAFETY: no other item in the link is named `luna_capi_panic_prepare`;
// `luna_throw` in `csrc/shim_core.c` is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_panic_prepare(L: *mut LuaState) {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let err = api.stack().last().copied().unwrap_or(Value::Nil);
    let err = match api.version() {
        LuaVersion::Lua52 => return,
        LuaVersion::Lua53 => {
            api.push(err);
            return;
        }
        LuaVersion::Lua51 => err,
        _ => match super::tbc::close_with(&mut api, 0, Some(err)) {
            Ok(()) => err,
            Err(e) => e.0,
        },
    };
    let s = api.st();
    s.calls.clear();
    s.base = 0;
    api.truncate(0);
    api.push(err);
}
