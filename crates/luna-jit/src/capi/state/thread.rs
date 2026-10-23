//! The `lua_State` of each thread, made the first time the C API sees
//! the thread.

use super::*;

/// PUC `LUA_EXTRASPACE`: raw memory just below the `lua_State` pointer,
/// for the host (`lua_getextraspace`).
pub(super) const EXTRASPACE: usize = std::mem::size_of::<*mut c_void>();

/// A `LuaState` with its extra space in front, as PUC's `LX`.
#[repr(C)]
pub(super) struct ThreadBox {
    extra: [u8; EXTRASPACE],
    st: LuaState,
}

/// The `LuaState` of `co` if C has asked for it before.
pub(super) fn existing(co: Gc<Coro>) -> Option<*mut LuaState> {
    let h = co.host_state()?;
    if !h.is::<ThreadBox>() {
        return None;
    }
    let b = h.as_ptr().cast::<ThreadBox>();
    // SAFETY: `b` is the live block holding the thread's `ThreadBox`, kept
    // by the thread as long as it lives
    Some(unsafe { &raw mut (*b).st })
}

/// PUC `LUA_MINSTACK`: the free slots a thread's stack has when C gets it
/// and when a C function starts, so that pushes within them need no memory.
pub(in crate::capi) const MINSTACK: usize = 20;

/// Make the `LuaState` of `co`, with `extra` as its extra space, in a
/// block of the state's allocation context that the thread keeps, and
/// room for `MINSTACK` values on its C stack; `None` when the memory for
/// them cannot be had.
pub(super) fn new_thread_state(
    g: *mut Global,
    co: Gc<Coro>,
    extra: [u8; EXTRASPACE],
) -> Option<*mut LuaState> {
    // SAFETY: `g` is the live global record of `co`'s state, whose Vm
    // pointer is live
    let mem = unsafe { (*(*g).vm).heap.mem() };
    let b = LAny::new(
        mem,
        ThreadBox {
            extra,
            st: LuaState {
                g,
                thread: co,
                base: 0,
                calls: LVec::new(mem),
                status: LUA_OK,
                pending_yield: None,
                hook: CHook {
                    strs: super::debug::CStrings::new(mem),
                    refs: super::debug::LevelRefs::new(mem),
                    ..CHook::default()
                },
                tbc: LVec::new(mem),
                parked: None,
            },
        },
        BlockKind::Thread,
    )
    .ok()?;
    // SAFETY: `co` is a live thread the caller holds and the Vm is not
    // touching it now; the borrow covers the reservation
    unsafe { co.as_mut() }
        .host_stack_mut()
        .reserve(MINSTACK)
        .ok()?;
    let p = b.as_ptr().cast::<ThreadBox>();
    // SAFETY: `co` is a live thread the caller holds and the Vm is not
    // touching it now; the borrow covers one store
    unsafe { co.as_mut() }.set_host_state(b);
    // SAFETY: `p` is the live block just stored in the thread
    Some(unsafe { &raw mut (*p).st })
}

/// The `lua_State` of thread `co` of the state `vm` belongs to, made when
/// the thread is (see `thread_created`) with a copy of the main thread's
/// extra space, as `lua_newthread` makes it. A coroutine that has not
/// started has its body on its stack, as `coroutine.create` leaves it in
/// PUC. Making it can fail only when memory runs out; the process ends then
/// (see [`try_state_of`]).
pub(in crate::capi) fn state_of(vm: &mut Vm, co: Gc<Coro>) -> *mut LuaState {
    try_state_of(vm, co)
        .unwrap_or_else(|| std::alloc::handle_alloc_error(std::alloc::Layout::new::<ThreadBox>()))
}

/// [`state_of`], `None` when the thread's state is not made yet and the
/// memory for it cannot be had.
pub(in crate::capi) fn try_state_of(vm: &mut Vm, co: Gc<Coro>) -> Option<*mut LuaState> {
    if let Some(l) = existing(co) {
        return Some(l);
    }
    let main = existing(vm.host_main_thread()).expect("the C API made this state");
    // SAFETY: `main` is the live main thread's state, and `ThreadBox` puts
    // `st` right after the extra space
    let (g, extra) = unsafe {
        let b = main.cast::<u8>().sub(EXTRASPACE).cast::<ThreadBox>();
        ((*main).g, (*b).extra)
    };
    let l = new_thread_state(g, co, extra)?;
    if !co.started && !co.body.is_nil() {
        // SAFETY: `co` is held by the caller and has not started, so no
        // context is loaded from it; the borrow covers one push, into the
        // room `new_thread_state` reserved
        unsafe { co.as_mut() }
            .host_stack_mut()
            .push_or_abort(co.body);
        vm.heap.barrier_back(co);
    }
    Some(l)
}
