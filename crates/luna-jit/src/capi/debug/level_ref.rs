//! Level references: what a `lua_Debug` holds to name a level of a thread.

use super::*;

/// The level index a level reference names on `api`'s thread, if it is a
/// level of it.
pub(in crate::capi) fn level_of_ref(api: &Api, r: usize) -> Option<usize> {
    let n = api.vm.host_level_count(api.thread());
    (r >= 1 && r <= n).then(|| n - r)
}

/// A level a `lua_Debug` points at from 5.2 on: the thread, as PUC's
/// `i_ci` names a `CallInfo` of whichever thread `lua_getstack` was asked
/// about, and the level reference on it.
pub(in crate::capi) struct LevelRef {
    l: *mut LuaState,
    r: usize,
}

/// The `LevelRef`s handed out for a thread, one per level reference, at
/// addresses that stay put as long as the thread, in blocks of the state's
/// allocation context. The default has no context and holds nothing (a
/// thread's own is made with [`LevelRefs::new`]).
#[derive(Default)]
pub(in crate::capi) struct LevelRefs(Option<LVec<LBox<LevelRef>>>);

impl LevelRefs {
    pub(in crate::capi) fn new(mem: MemRef) -> LevelRefs {
        LevelRefs(Some(LVec::new(mem)))
    }
}

/// What `i_ci` holds for level reference `r` of `l`'s thread: 5.1's is an
/// `int` relative to the thread its functions are given, as PUC's.
/// `None` when there is no memory to keep the reference (the functions
/// handing one out cannot raise, and report no such level then).
pub(in crate::capi) fn encode_ref(api: &mut Api, l: *mut LuaState, r: usize) -> Option<usize> {
    if api.version() == LuaVersion::Lua51 {
        return Some(r);
    }
    // SAFETY: `l` is a live thread of the state; the borrow ends here
    let refs = unsafe { &mut (*l).hook.refs }.0.as_mut()?;
    if let Some(b) = refs.iter().find(|b| b.r == r) {
        return Some(std::ptr::from_ref::<LevelRef>(b) as usize);
    }
    refs.reserve(1).ok()?;
    let b = LBox::new(refs.mem(), LevelRef { l, r }).ok()?;
    let p = std::ptr::from_ref::<LevelRef>(&b) as usize;
    refs.push_or_abort(b);
    Some(p)
}

/// The thread and level reference `i_ci` holds; `None` for 5.1's lost
/// tail call.
pub(super) fn decode_ref(api: &Api, raw: usize) -> Option<(*mut LuaState, usize)> {
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
