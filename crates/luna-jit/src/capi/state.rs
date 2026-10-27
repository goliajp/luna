//! States and threads: `lua_State`, the record shared by every thread of a
//! state, and making and closing states.

use super::ccall::{CCall, CHook, PendingYield};
use super::*;
use luna_core::runtime::mem::{BlockKind, LAny, MemOwner};

mod make;
mod thread;
pub use make::*;
pub(super) use thread::*;

/// PUC `lua_Alloc`: `(ud, ptr, osize, nsize)`.
pub type LuaAlloc = unsafe extern "C" fn(*mut c_void, *mut c_void, usize, usize) -> *mut c_void;

/// What every thread of a state shares (PUC `global_State`). The first
/// five fields are read and written by the C side (`struct luna_G` in
/// `csrc/shim.h`), in this order.
#[repr(C)]
pub(crate) struct Global {
    /// innermost error boundary (`struct luna_jmp *`), null outside any
    pub(super) errjmp: *mut c_void,
    /// a status a Rust API function asks its C caller to throw
    pub(super) raised: c_int,
    /// `LUA_VERSION_NUM` of the dialect
    pub(super) version: c_int,
    /// `lua_atpanic`'s function
    pub(super) panic: Option<LuaCFunction>,
    /// the thread whose stack top is the error being thrown
    pub(super) err_from: *mut LuaState,
    /// The Vm, through the pointer the innermost Rust caller of C handed
    /// down: a call into C stores the pointer derived from its own
    /// `&mut Vm` here and puts the previous one back after, so what C does
    /// to the Vm goes through the reference the Rust code above it holds.
    pub(super) vm: *mut Vm,
    /// the Vm's block from the allocation function, freed by `lua_close`
    vm_box: *mut Vm,
    /// `lua_newstate`'s allocation function, which `lua_getallocf` returns
    pub(super) alloc: Option<LuaAlloc>,
    pub(super) alloc_ud: *mut c_void,
    /// the debug library's hook as `lua_gethook` handed it out
    pub(super) hookf: Option<super::ccall::LuaHook>,
}

/// A thread as C sees it (PUC `lua_State`). The first field is read by the
/// C side (`struct luna_L`).
#[repr(C)]
pub struct LuaState {
    pub(super) g: *mut Global,
    /// the thread's identity: a coroutine, or the main thread's object
    pub(super) thread: Gc<Coro>,
    /// where the running C function's values start in the thread's C
    /// stack: its index 1
    pub(super) base: usize,
    /// the C functions running on this thread or waiting on a
    /// continuation, innermost last
    pub(super) calls: LVec<CCall>,
    /// PUC `L->status`: `LUA_YIELD` while suspended, the error status a
    /// resume ended with, else `LUA_OK`
    pub(super) status: c_int,
    /// a `lua_yield` made by the running C function, taken when it leaves
    pub(super) pending_yield: Option<PendingYield>,
    /// this thread's C hook (`lua_sethook`)
    pub(super) hook: CHook,
    /// C stack indices of its to-be-closed slots, ascending
    pub(super) tbc: LVec<usize>,
    /// after a yield from Lua code that a resume from C reported: where
    /// the yielded values start on the C stack, and the base to put back
    /// when the thread is resumed
    pub(super) parked: Option<(usize, usize)>,
}

// SAFETY: the declarations match the definitions in `csrc/shim_core.c`;
// each takes plain values and touches only the C library's stdout
unsafe extern "C" {
    safe fn luna_c_stdout_write(p: *const c_void, n: usize) -> c_int;
    safe fn luna_c_stdout_flush() -> c_int;
    safe fn luna_c_stdout_setvbuf(mode: c_int);
}

fn host_stdout_write(b: &[u8]) -> bool {
    luna_c_stdout_write(b.as_ptr().cast(), b.len()) != 0
}

fn host_stdout_flush() -> bool {
    luna_c_stdout_flush() != 0
}

fn host_stdout_setvbuf(mode: u8) {
    luna_c_stdout_setvbuf(c_int::from(mode.min(2)));
}

// `lua_Alloc` blocks are aligned for any C object, which is all the
// alignment these records need
const _: () = assert!(std::mem::align_of::<Vm>() <= 8 && std::mem::align_of::<Global>() <= 8);

/// A new state of dialect `v`, with luna's JIT and the C API's
/// continuation hooks installed, or null when `alloc` cannot allocate the
/// state's record: as PUC's `lua_newstate`, every block of the state comes
/// from the host's allocation function and goes back to it, the state's
/// record and the Vm's own block on `lua_close`. From the first state on,
/// luna's standard output goes through the C library's `stdout`, as PUC's
/// does. Strings hash with `seed`, or with a seed of the state's own.
fn new_state(v: LuaVersion, alloc: LuaAlloc, ud: *mut c_void, seed: Option<u32>) -> *mut LuaState {
    // PUC passes the kind of object as the old size of a new block: 5.1
    // passes 0, later versions LUA_TTHREAD for the main block
    let kind = if v == LuaVersion::Lua51 { 0 } else { 8 };
    // SAFETY: `alloc` is the host's `lua_Alloc`, called as PUC calls it for
    // a new block
    let block = unsafe {
        alloc(
            ud,
            std::ptr::null_mut(),
            kind,
            std::mem::size_of::<Global>(),
        )
    };
    if block.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: as above, for the Vm's block, which is not an object
    let vm_block = unsafe { alloc(ud, std::ptr::null_mut(), 0, std::mem::size_of::<Vm>()) };
    if vm_block.is_null() {
        // SAFETY: `block` is the live block just allocated, of this size
        unsafe { alloc(ud, block, std::mem::size_of::<Global>(), 0) };
        return std::ptr::null_mut();
    }
    luna_core::__private::use_host_stdout(luna_core::__private::HostStdout {
        write: host_stdout_write,
        flush: host_stdout_flush,
        setvbuf: host_stdout_setvbuf,
    });
    // SAFETY: `alloc` follows the `lua_Alloc` contract (the host's promise
    // to `lua_newstate`), and the Vm, which owns the context, is dropped
    // by `lua_close` before the host may stop accepting calls
    let mem = unsafe { MemOwner::raw(alloc, ud, v) };
    mem.ctx()
        .add_external(std::mem::size_of::<Global>() + std::mem::size_of::<Vm>());
    let vm = match seed {
        Some(s) => Vm::try_new_minimal_with_mem_and_seed(v, mem, s),
        None => Vm::try_new_minimal_with_mem(v, mem),
    };
    let Some(mut vm) = vm else {
        // SAFETY: both blocks are live, of these sizes, and unused
        unsafe {
            alloc(ud, vm_block, std::mem::size_of::<Vm>(), 0);
            alloc(ud, block, std::mem::size_of::<Global>(), 0);
        }
        return std::ptr::null_mut();
    };
    crate::install_default_jit(&mut vm);
    vm.set_host_cont_hooks(ccall::CONT_HOOKS);
    vm.host_gc_start_incremental();
    let vm_ptr = vm_block.cast::<Vm>();
    // SAFETY: `vm_block` is a fresh block of the Vm's size, aligned as
    // `lua_Alloc` blocks are for any object, which nothing else uses
    unsafe { vm_ptr.write(vm) };
    let g = block.cast::<Global>();
    // SAFETY: `block` is a fresh allocation of `Global`'s size, aligned as
    // `lua_Alloc` blocks are for any object, which nothing else uses
    unsafe {
        g.write(Global {
            errjmp: std::ptr::null_mut(),
            raised: 0,
            version: version_num(v),
            panic: None,
            err_from: std::ptr::null_mut(),
            vm: vm_ptr,
            vm_box: vm_ptr,
            alloc: Some(alloc),
            alloc_ud: ud,
            hookf: None,
        })
    };
    // SAFETY: `vm_ptr` was just written and nothing else refers to it yet
    let vmr = unsafe { &mut *vm_ptr };
    let main = vmr.host_main_thread();
    let l = match vmr.host_make_registry() {
        Ok(_) => new_thread_state(g, main, [0; EXTRASPACE]),
        Err(_) => None,
    };
    match l {
        Some(l) => l,
        None => {
            // SAFETY: nothing but this function has seen the state: the Vm
            // is dropped, freeing every block it allocated, then the two
            // blocks go back to the host's function
            unsafe {
                std::ptr::drop_in_place(vm_ptr);
                alloc(ud, vm_block, std::mem::size_of::<Vm>(), 0);
                alloc(ud, block, std::mem::size_of::<Global>(), 0);
            }
            std::ptr::null_mut()
        }
    }
}

/// Close the state (PUC `lua_close`): the main thread's to-be-closed
/// slots are closed and every finalizer runs, their errors dropped, then
/// the Vm is freed and the state's record goes back to the allocation
/// function. A null pointer is a no-op; `L` may be any thread of the state.
///
/// # Safety
/// `L` is null or a thread of a state that has not been closed, and no
/// API call on the state is running.
// SAFETY: no other item in the link is named `lua_close`: the host does not
// link PUC's liblua next to this crate, which defines each `lua_*` symbol
// once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lua_close(L: *mut LuaState) {
    if L.is_null() {
        return;
    }
    // SAFETY: `L` is a live thread of an open state (# Safety), so its
    // global record and Vm are live, and no call is using them: the Vm
    // pointer is the Vm's own allocation
    unsafe {
        let g = (*L).g;
        let vmr = &mut *(*g).vm_box;
        let mt = vmr.host_main_thread();
        let main = state_of(vmr, mt);
        let mut api = Api::new(main);
        let _ = super::tbc::close_with(&mut api, 0, None);
        let vm = &mut *(*g).vm_box;
        vm.host_close_state();
        // the Vm owns every thread, and so every `LuaState`
        std::ptr::drop_in_place((*g).vm_box);
        let (alloc, ud) = ((*g).alloc, (*g).alloc_ud);
        if let Some(f) = alloc {
            f(ud, (*g).vm_box.cast(), std::mem::size_of::<Vm>(), 0);
            f(ud, g.cast(), std::mem::size_of::<Global>(), 0);
        }
    }
}
