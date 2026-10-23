//! Vms whose allocations are refused from some point on, so that the
//! collector runs its emergency collections.
//!
//! The Vm runs on memory that is never reused: a freed block is filled
//! with a byte no live length or pointer reads as, and kept, and checked
//! at the end to have been written by nothing (marking a freed object
//! writes its color).

use std::alloc::{Layout, alloc, dealloc};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use luna_core::runtime::Value;
use luna_core::runtime::mem::MemOwner;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const ALIGN: usize = 16;
const POISON: u8 = 0xDB;

/// The freed blocks kept, and the growths counted and refused: every one
/// from the `.1.1`th on.
struct Held(RefCell<Vec<(*mut u8, usize)>>, Cell<(usize, usize)>);

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
            unsafe { ptr.write_bytes(POISON, osize) };
            held.0.borrow_mut().push((ptr, osize));
        }
        return std::ptr::null_mut();
    }
    if nsize > osize {
        let (grown, refuse_from) = held.1.get();
        held.1.set((grown + 1, refuse_from));
        if grown + 1 >= refuse_from {
            return std::ptr::null_mut();
        }
    }
    let layout = Layout::from_size_align(nsize, ALIGN).expect("a block size");
    // SAFETY: the size is not 0
    let p = unsafe { alloc(layout) };
    if !ptr.is_null() && !p.is_null() {
        // SAFETY: both blocks are live and at least this long; the old one
        // came from here with `osize`, and is kept, poisoned
        unsafe {
            std::ptr::copy_nonoverlapping(ptr, p, osize.min(nsize));
            ptr.write_bytes(POISON, osize);
        }
        held.0.borrow_mut().push((ptr, osize));
    }
    p as *mut c_void
}

/// `source` on a Vm of `version` with its libraries open, every growth
/// from the `refuse_from`th on (counted from when `source` starts) refused.
pub(crate) fn run(version: LuaVersion, seed: u32, refuse_from: usize, source: &str) -> String {
    let held = Box::into_raw(Box::new(Held(
        RefCell::default(),
        Cell::new((0, usize::MAX)),
    )));
    // SAFETY: `quarantine` follows the `lua_Alloc` contract and `held`
    // outlives the Vm, dropped below before it
    let mem = unsafe { MemOwner::raw(quarantine, held as *mut c_void, version) };
    let mut vm = Vm::try_new_minimal_with_mem_and_seed(version, mem, seed).expect("a Vm");
    vm.open_all_libs().expect("the libraries open");
    // SAFETY: `held` lives until after the Vm is dropped below
    unsafe { &*held }.1.set((0, refuse_from));
    // loaded and called as a host does (`luaL_loadstring`, `lua_pcall`)
    let called = vm
        .load(source.as_bytes(), b"=chunk")
        .map_err(|e| e.msg_str().to_string())
        .and_then(|f| {
            vm.call_value(Value::Closure(f), &[])
                .map_err(|e| vm.error_display(&e))
        });
    let out = match called {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            Some(Value::Int(i)) => i.to_string(),
            // 5.1's numbers are all floats
            Some(Value::Float(f)) if f.fract() == 0.0 => (*f as i64).to_string(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {e}"),
    };
    drop(vm);
    // SAFETY: made above; the Vm, its only user, is gone
    let held = unsafe { Box::from_raw(held) };
    let blocks = held.0.take();
    // a freed block written since (a collection marking an object freed
    // before) is a use after free
    let written = blocks.iter().any(|&(p, n)| {
        // SAFETY: each block came from `quarantine` with this size and is kept
        unsafe { std::slice::from_raw_parts(p, n) }
            .iter()
            .any(|&b| b != POISON)
    });
    for &(p, n) in &blocks {
        // SAFETY: each block came from `quarantine` with this size
        unsafe { dealloc(p, Layout::from_size_align_unchecked(n, ALIGN)) };
    }
    if written {
        return format!("a freed block written: {out}");
    }
    out
}
