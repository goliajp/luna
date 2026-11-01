//! The debug interface: stack levels, function information, locals,
//! upvalues.

use super::api::C_UPVALS;
use super::*;
use luna_core::__private::{HostAr, HostLevel};
use luna_core::runtime::NativeClosure;
use luna_core::runtime::mem::{LBox, LVec};

mod ar;
mod cstrings;
pub(super) mod upvals;

pub(super) use ar::{ArBuf, DebugPtr};
pub(super) use cstrings::CStrings;

mod level_ref;
mod locals;
pub(super) use level_ref::*;
use locals::c_name;

/// The C API's function behind `f`, if it is one.
pub(super) fn c_api_fn(f: Value) -> Option<Gc<NativeClosure>> {
    match f {
        Value::Native(nc) if nc.builtin == luna_core::__private::Builtin::CFunction => Some(nc),
        _ => None,
    }
}

/// The `lua_Debug` at `ar`, of the state's dialect.
fn debug_ptr(api: &Api, ar: *mut c_void) -> DebugPtr {
    // SAFETY: the API functions taking `ar` require a `lua_Debug` of the
    // state's dialect there (PUC's contract)
    unsafe { DebugPtr::new(api.version(), ar) }
}

/// The level `ar` points at: its thread's API context and level index.
fn target<'a>(api: &'a mut Api, d: DebugPtr) -> Option<(Api<'a>, usize, usize)> {
    let (l, r) = decode_ref(api, d.level_ref())?;
    let t = Api {
        vm: &mut *api.vm,
        l,
    };
    let i = level_of_ref(&t, r)?;
    Some((t, i, r))
}

/// PUC `lua_getstack`: point `ar` at level `level` of the thread; 0 when
/// it has no such level. 5.1 answers a negative level as a lost tail call.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into; `ar` is a writable
/// `lua_Debug` of the state's dialect.
// SAFETY: no other item in the link is named `lua_getstack`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getstack(L: *mut LuaState, level: c_int, ar: *mut c_void) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let d = debug_ptr(&api, ar);
    let Ok(level) = usize::try_from(level) else {
        if api.version() == LuaVersion::Lua51 {
            d.set_level_ref(0);
            return 1;
        }
        return 0;
    };
    let n = api.vm.host_level_count(api.thread());
    if level >= n {
        return 0;
    }
    let Some(raw) = encode_ref(&mut api, L, n - level) else {
        return 0;
    };
    d.set_level_ref(raw);
    1
}

/// What `lua_getinfo` reports of level `i`, with what the C API knows
/// beyond luna's levels: a C function's upvalue count, and the values a
/// call or return hook transfers to the level it interrupted.
fn level_info(api: &mut Api, i: usize, r: usize) -> HostAr {
    let co = api.thread();
    let mut info = api.vm.host_level_info(co, i);
    fix_c(&mut info);
    let hooked = api.st().hook.running.as_ref().is_some_and(|h| h.level == r);
    if hooked {
        let (f, n) = api.vm.host_transfer();
        if api.version() >= LuaVersion::Lua55 || n != 0 {
            (info.ftransfer, info.ntransfer) = (f, n);
        }
    }
    info
}

/// A C function's upvalues follow the C API's own (`C_UPVALS`).
fn fix_c(info: &mut HostAr) {
    if let Some(nc) = c_api_fn(info.func) {
        info.nups = (nc.upvals.len() - C_UPVALS) as i64;
    }
}

/// The option letters of each dialect.
fn valid(v: LuaVersion, c: u8) -> bool {
    let opts: &[u8] = match v {
        LuaVersion::Lua51 => b"SlunLf",
        LuaVersion::Lua52 | LuaVersion::Lua53 => b"SlutnLf",
        _ => b"SlutnrLf",
    };
    opts.contains(&c)
}

/// PUC `lua_getinfo`: fill `ar` with what the options in `what` ask for, of the level `ar` points
/// at, or with `>`, of the function popped from the stack. `f` pushes the function and `L` its
/// lines. Returns 0 when an option is not one of the dialect's.
///
/// # Safety
/// As [`lua_getstack`]; `what` is a NUL-terminated string, and `ar` was
/// filled by `lua_getstack` or a hook unless `what` starts with `>`.
// SAFETY: no other item in the link is named `lua_getinfo`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getinfo(
    L: *mut LuaState,
    what: *const c_char,
    ar: *mut c_void,
) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    // SAFETY: `what` is NUL-terminated (# Safety)
    let mut opts = unsafe { c_bytes(what) }.unwrap_or_default();
    let v = api.version();
    let d = debug_ptr(&api, ar);
    let info = if opts.first() == Some(&b'>') {
        opts = &opts[1..];
        let f = api.pop();
        let mut info = api.vm.host_function_info(f);
        fix_c(&mut info);
        info
    } else {
        match target(&mut api, d) {
            Some((mut t, i, r)) => level_info(&mut t, i, r),
            None => api.vm.host_tail_info(),
        }
    };
    let tail = info.what == "tail";
    let mut status = 1;
    let mut strs = std::mem::take(&mut api.st().hook.strs);
    if tail && v == LuaVersion::Lua51 {
        d.fill_tail(&info, &mut strs);
    } else {
        for &c in opts {
            if !valid(v, c) || !d.fill(c, &info, &mut strs) {
                status = 0;
            }
        }
    }
    api.st().hook.strs = strs;
    if opts.contains(&b'f') {
        api.push(info.func);
    }
    if opts.contains(&b'L') {
        let lines = api.vm.host_active_lines(info.func);
        api.push(lines);
    }
    status
}

/// PUC 5.1 `lua_setlevel`: carry the C call depth of `from` over to `to`.
/// luna counts C calls per state, so there is nothing to carry.
///
/// # Safety
/// `from` and `to` are live threads of open states.
// SAFETY: no other item in the link is named `lua_setlevel`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setlevel(from: *mut LuaState, to: *mut LuaState) {
    let _ = (from, to);
}
