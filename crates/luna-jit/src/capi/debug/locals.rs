//! `lua_getlocal` and `lua_setlocal`: a level's locals, and a C function's
//! values on its stack.

use super::*;

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
pub(super) fn c_name(api: &mut Api, name: &[u8]) -> *const c_char {
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
                // SAFETY: `co` is a live thread of this Vm; the heap is written in place
                unsafe { t.vm.heap_mut().barrier_back(co) };
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
