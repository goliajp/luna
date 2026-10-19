//! A memory context that never reuses a freed block: it zeroes the block
//! and keeps it until the Vm is gone. A read of a freed object then finds
//! zeros, the same every run, rather than whatever the allocator put there.

use std::alloc::{Layout, alloc, dealloc};
use std::cell::RefCell;
use std::ffi::c_void;

use luna_jit::runtime::mem::MemOwner;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const ALIGN: usize = 16;

#[derive(Default)]
struct Held(RefCell<Vec<(*mut u8, usize)>>);

unsafe extern "C" fn quarantine(
    ud: *mut c_void,
    ptr: *mut c_void,
    osize: usize,
    nsize: usize,
) -> *mut c_void {
    // SAFETY: `ud` is the `Held` the run made, alive until its Vm is dropped
    let held = unsafe { &*(ud as *const Held) };
    let ptr = ptr as *mut u8;
    if nsize == 0 {
        if !ptr.is_null() {
            // SAFETY: a live block of `osize` bytes from here
            unsafe { ptr.write_bytes(0, osize) };
            held.0.borrow_mut().push((ptr, osize));
        }
        return std::ptr::null_mut();
    }
    let layout = Layout::from_size_align(nsize, ALIGN).expect("a block size");
    // SAFETY: the size is not 0
    let p = unsafe { alloc(layout) };
    if !ptr.is_null() && !p.is_null() {
        // SAFETY: both blocks are live and at least this long; the old one
        // came from here with `osize`
        unsafe {
            std::ptr::copy_nonoverlapping(ptr, p, osize.min(nsize));
            dealloc(ptr, Layout::from_size_align_unchecked(osize, ALIGN));
        }
    }
    p as *mut c_void
}

/// Run `f` on a minimal Vm of `version` and hash seed `seed` whose freed
/// blocks are zeroed and never reused.
pub(super) fn with_quarantine<R>(
    version: LuaVersion,
    seed: u32,
    f: impl FnOnce(&mut Vm) -> R,
) -> R {
    let held = Box::into_raw(Box::new(Held::default()));
    // SAFETY: `quarantine` follows the `lua_Alloc` contract and `held`
    // outlives the Vm, dropped below before it
    let mem = unsafe { MemOwner::raw(quarantine, held as *mut c_void, version) };
    let mut vm = Vm::try_new_minimal_with_mem_and_seed(version, mem, seed).expect("a Vm");
    let r = f(&mut vm);
    drop(vm);
    // SAFETY: made above; the Vm, the only user, is gone
    let held = unsafe { Box::from_raw(held) };
    for (p, n) in held.0.take() {
        // SAFETY: each block came from `quarantine` with this size
        unsafe { dealloc(p, Layout::from_size_align_unchecked(n, ALIGN)) };
    }
    r
}
