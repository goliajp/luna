//! The entry points that make a state: PUC's `luaL_newstate` and
//! `lua_newstate`, the per-dialect forms luna's headers map them to, and
//! luna's forms of both that take the strings' hash seed from the caller.

use super::*;

/// The dialect whose `LUA_VERSION_NUM` is `n`.
fn dialect(n: c_int) -> Option<LuaVersion> {
    Some(match n {
        501 => LuaVersion::Lua51,
        502 => LuaVersion::Lua52,
        503 => LuaVersion::Lua53,
        504 => LuaVersion::Lua54,
        505 => LuaVersion::Lua55,
        _ => return None,
    })
}

/// A 5.5 state with luaL_newstate's panic function (PUC `luaL_newstate`).
/// A host compiled against luna's headers gets its own dialect: the
/// headers make `luaL_newstate()` a call of `luna_newstate`.
// SAFETY: no other item in the link is named `luaL_newstate`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub extern "C" fn luaL_newstate() -> *mut LuaState {
    luna_newstate(505)
}

/// `luaL_newstate` for the dialect whose `LUA_VERSION_NUM` is `version`
/// (501 to 505); null for any other number. Its allocation function is
/// `luaL_alloc`, as PUC's.
// SAFETY: no other item in the link is named `luna_newstate`: PUC's liblua
// has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub extern "C" fn luna_newstate(version: c_int) -> *mut LuaState {
    aux_state(version, None)
}

/// [`luna_newstate`] whose strings hash with `seed`: two states made with
/// the same seed lay out their tables, and so iterate them, the same way.
/// 5.1 hashes strings without a seed.
// SAFETY: no other item in the link is named `luna_newstate_seeded`: PUC's
// liblua has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub extern "C" fn luna_newstate_seeded(version: c_int, seed: u32) -> *mut LuaState {
    aux_state(version, Some(seed))
}

fn aux_state(version: c_int, seed: Option<u32>) -> *mut LuaState {
    let Some(v) = dialect(version) else {
        return std::ptr::null_mut();
    };
    let l = new_state(v, luna_c_l_alloc, std::ptr::null_mut(), seed);
    if l.is_null() {
        return l;
    }
    // SAFETY: `l` is the main thread's state just made; `g` is its global
    // record, and nothing runs on the state yet
    unsafe {
        let g = (*l).g;
        (*g).panic = Some(luna_c_default_panic);
        // 5.4 starts with warnings off, 5.5 on
        if v >= LuaVersion::Lua55 {
            (*(*g).vm).host_warn_on();
        }
    }
    l
}

/// PUC `lua_newstate` for the dialect `version`: a state without a panic
/// function or a warning function, null when `f` fails to allocate the
/// state's record. `f` allocates that record only; luna's collector does
/// not allocate through it. `seed` is 5.5's: strings hash with it, as
/// PUC 5.5's do; earlier dialects pick a seed of the state's own.
// SAFETY: no other item in the link is named `luna_newstate_with`: PUC's
// liblua has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub extern "C" fn luna_newstate_with(
    version: c_int,
    f: Option<LuaAlloc>,
    ud: *mut c_void,
    seed: u32,
) -> *mut LuaState {
    let seed = (version >= 505).then_some(seed);
    host_state(version, f, ud, seed)
}

/// [`luna_newstate_with`] whose strings hash with `seed` in every dialect
/// but 5.1, which hashes strings without one.
// SAFETY: no other item in the link is named `luna_newstate_with_seeded`:
// PUC's liblua has no such symbol and this crate defines it once
#[unsafe(no_mangle)]
pub extern "C" fn luna_newstate_with_seeded(
    version: c_int,
    f: Option<LuaAlloc>,
    ud: *mut c_void,
    seed: u32,
) -> *mut LuaState {
    host_state(version, f, ud, Some(seed))
}

fn host_state(
    version: c_int,
    f: Option<LuaAlloc>,
    ud: *mut c_void,
    seed: Option<u32>,
) -> *mut LuaState {
    let (Some(v), Some(f)) = (dialect(version), f) else {
        return std::ptr::null_mut();
    };
    let l = new_state(v, f, ud, seed);
    if !l.is_null() {
        // SAFETY: `l` is the main thread's state just made, and nothing
        // runs on the state yet
        let vm = unsafe { &mut *(*(*l).g).vm };
        vm.set_host_warn(Some(Box::new(|_, _, _| Ok(()))));
    }
    l
}

/// PUC 5.5 `lua_newstate`.
// SAFETY: no other item in the link is named `lua_newstate`: the host does
// not link PUC's liblua next to this crate, which defines each `lua_*`
// symbol once
#[unsafe(no_mangle)]
pub extern "C" fn lua_newstate(f: Option<LuaAlloc>, ud: *mut c_void, seed: u32) -> *mut LuaState {
    luna_newstate_with(505, f, ud, seed)
}

// SAFETY: the declaration matches the definition in `csrc/shim_core.c`;
// it is `safe` to name because it is only stored as a panic function,
// which luna calls only with a live state. C sees `lua_State` as opaque
// and reads only its leading fields, which `csrc/shim.h` declares
#[allow(improper_ctypes)]
unsafe extern "C" {
    /// luaL_newstate's panic function.
    safe fn luna_c_default_panic(L: *mut LuaState) -> c_int;
    /// luaL_newstate's allocation function (PUC `l_alloc`).
    fn luna_c_l_alloc(ud: *mut c_void, ptr: *mut c_void, osize: usize, nsize: usize)
    -> *mut c_void;
}
