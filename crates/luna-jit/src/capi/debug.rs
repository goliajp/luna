//! The debug interface: stack levels, function information, locals,
//! upvalues.

use super::api::C_UPVALS;
use super::*;
use luna_core::runtime::NativeClosure;
use luna_core::vm::exec::host_c::{HostAr, HostLevel};

mod ar;
pub(super) mod upvals;

pub(super) use ar::{ArBuf, CStrings, DebugPtr};

/// The C API's function behind `f`, if it is one.
pub(super) fn c_api_fn(f: Value) -> Option<Gc<NativeClosure>> {
    let trampoline: luna_core::runtime::value::NativeFn = ccall::capi_trampoline;
    match f {
        Value::Native(nc) if std::ptr::fn_addr_eq(nc.f, trampoline) => Some(nc),
        _ => None,
    }
}

/// The `lua_Debug` at `ar`, of the state's dialect.
fn debug_ptr(api: &Api, ar: *mut c_void) -> DebugPtr {
    // SAFETY: the API functions taking `ar` require a `lua_Debug` of the
    // state's dialect there (PUC's contract)
    unsafe { DebugPtr::new(api.version(), ar) }
}

/// The level index a level reference names on `api`'s thread, if it is a
/// level of it.
pub(super) fn level_of_ref(api: &Api, r: usize) -> Option<usize> {
    let n = api.vm.host_level_count(api.thread());
    (r >= 1 && r <= n).then(|| n - r)
}

/// A level a `lua_Debug` points at from 5.2 on: the thread, as PUC's
/// `i_ci` names a `CallInfo` of whichever thread `lua_getstack` was asked
/// about, and the level reference on it.
pub(super) struct LevelRef {
    l: *mut LuaState,
    r: usize,
}

/// The `LevelRef`s handed out for a thread, one per level reference, at
/// addresses that stay put as long as the thread.
#[derive(Default)]
pub(super) struct LevelRefs(std::collections::HashMap<usize, Box<LevelRef>>);

/// What `i_ci` holds for level reference `r` of `l`'s thread: 5.1's is an
/// `int` relative to the thread its functions are given, as PUC's.
pub(super) fn encode_ref(api: &mut Api, l: *mut LuaState, r: usize) -> usize {
    if api.version() == LuaVersion::Lua51 {
        return r;
    }
    // SAFETY: `l` is a live thread of the state; the borrow ends here
    let refs = unsafe { &mut (*l).hook.refs };
    let b = refs
        .0
        .entry(r)
        .or_insert_with(|| Box::new(LevelRef { l, r }));
    std::ptr::from_ref::<LevelRef>(b) as usize
}

/// The thread and level reference `i_ci` holds; `None` for 5.1's lost
/// tail call.
fn decode_ref(api: &Api, raw: usize) -> Option<(*mut LuaState, usize)> {
    if api.version() == LuaVersion::Lua51 {
        return Some((api.l, raw));
    }
    if raw == 0 {
        return None;
    }
    // SAFETY: a nonzero `i_ci` of 5.2+ is the address of a `LevelRef` that
    // `encode_ref` keeps with its thread (PUC's contract: `ar` was filled by
    // `lua_getstack` or a hook while the thread lives)
    let lr = unsafe { &*(raw as *const LevelRef) };
    Some((lr.l, lr.r))
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
    let raw = encode_ref(&mut api, L, n - level);
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

/// PUC `lua_getinfo`: fill `ar` with what the options in `what` ask for,
/// of the level `ar` points at, or with `>`, of the function popped from
/// the stack. `f` pushes the function and `L` its lines. Returns 0 when an
/// option is not one of the dialect's.
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

/// The values of the C function of the C API running at Lua stack slot
/// `func_slot` of `api`'s thread: their range in the thread's C stack.
fn c_frame(api: &mut Api, func_slot: u32) -> Option<(usize, usize)> {
    let top = api.top();
    let s = api.st();
    let k = s.calls.iter().position(|c| c.func_slot == func_slot)?;
    let base = s.calls[k].base;
    // its values end where the next frame on the C stack starts: a C
    // function it called, or a hook running above it
    let limit = match s.calls.get(k + 1) {
        Some(c) => c.base,
        None if s.base > base => s.base,
        None => top,
    };
    Some((base, limit))
}

/// Where local `n` of the level `ar` points at lives: a slot of a C
/// function's C stack frame, or one luna's levels know.
enum Local {
    CSlot(usize),
    Level(usize),
}

/// Local `n` of level `i` of `api`'s thread.
fn find_local(api: &mut Api, i: usize, n: c_int) -> Option<(Local, Vec<u8>)> {
    let co = api.thread();
    if let Some(HostLevel::C { func, func_slot }) = api.vm.host_level(co, i)
        && c_api_fn(func).is_some()
        && let Some((base, limit)) = c_frame(api, func_slot)
    {
        let k = usize::try_from(n)
            .ok()
            .filter(|&k| k >= 1 && base + k <= limit)?;
        let name = api.vm.host_c_temporary_name().as_bytes().to_vec();
        return Some((Local::CSlot(base + k - 1), name));
    }
    let (name, _) = api.vm.host_local(co, i, i64::from(n))?;
    Some((Local::Level(i), name.into_bytes()))
}

/// A name handed to C, kept with the thread's other debug strings.
fn c_name(api: &mut Api, name: &[u8]) -> *const c_char {
    let mut strs = std::mem::take(&mut api.st().hook.strs);
    let p = strs.get(name);
    api.st().hook.strs = strs;
    p
}

/// PUC `lua_getlocal`: push local `n` of the level `ar` points at and
/// return its name, or return NULL. With `ar` NULL (5.2+), the name of
/// parameter `n` of the Lua function on top of the stack, pushing nothing.
///
/// # Safety
/// As [`lua_getstack`]; `ar` is NULL or was filled by `lua_getstack` or a
/// hook.
// SAFETY: no other item in the link is named `lua_getlocal`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_getlocal(
    L: *mut LuaState,
    ar: *const c_void,
    n: c_int,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    if ar.is_null() {
        let f = api.get_or_nil(-1);
        return match api.vm.host_param_name(f, i64::from(n)) {
            Some(name) => c_name(&mut api, name.as_bytes()),
            None => std::ptr::null(),
        };
    }
    let d = debug_ptr(&api, ar.cast_mut());
    let (v, name) = {
        let Some((mut t, i, _)) = target(&mut api, d) else {
            return std::ptr::null();
        };
        let Some((at, name)) = find_local(&mut t, i, n) else {
            return std::ptr::null();
        };
        let v = match at {
            Local::CSlot(s) => t.stack()[s],
            Local::Level(i) => {
                let co = t.thread();
                t.vm.host_local(co, i, i64::from(n))
                    .map_or(Value::Nil, |(_, v)| v)
            }
        };
        (v, name)
    };
    api.push(v);
    c_name(&mut api, &name)
}

/// PUC `lua_setlocal`: pop the top value into local `n` of the level `ar`
/// points at and return its name, or return NULL. 5.1 and 5.2 pop the
/// value either way.
///
/// # Safety
/// As [`lua_getlocal`]; `ar` is not NULL.
// SAFETY: no other item in the link is named `lua_setlocal`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_setlocal(
    L: *mut LuaState,
    ar: *const c_void,
    n: c_int,
) -> *const c_char {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    let d = debug_ptr(&api, ar.cast_mut());
    let v = api.get_or_nil(-1);
    let always_pop = api.version() <= LuaVersion::Lua52;
    let found = target(&mut api, d).and_then(|(mut t, i, _)| {
        let (at, name) = find_local(&mut t, i, n)?;
        match at {
            Local::CSlot(s) => {
                t.stack_mut()[s] = v;
                let co = t.thread();
                t.vm.heap.barrier_back(co);
            }
            Local::Level(i) => {
                let co = t.thread();
                t.vm.host_set_local(co, i, i64::from(n), v);
            }
        }
        Some(name)
    });
    let Some(name) = found else {
        if always_pop {
            api.pop();
        }
        return std::ptr::null();
    };
    api.pop();
    c_name(&mut api, &name)
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
