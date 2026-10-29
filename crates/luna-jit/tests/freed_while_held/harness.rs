//! The Vm the tests run on: every library open, the trace JIT recording
//! after a given number of back edges, memory that is never reused (a
//! freed block filled with [`POISON`] and kept until the Vm is gone), and
//! any panic in the run a failure.

use std::alloc::{Layout, alloc, dealloc};
use std::cell::RefCell;
use std::ffi::c_void;

use luna_core::__private::{VmJitState, panics_seen, watch_panics};
use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::runtime::mem::MemOwner;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const ALIGN: usize = 16;

/// What a freed block is filled with: no length, pointer or tag of a live
/// object reads as it.
const POISON: u8 = 0xDB;

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
            unsafe { ptr.write_bytes(POISON, osize) };
            held.0.borrow_mut().push((ptr, osize));
        }
        return std::ptr::null_mut();
    }
    let layout = Layout::from_size_align(nsize, ALIGN).expect("a block size");
    // SAFETY: the size is not 0
    let p = unsafe { alloc(layout) };
    if !ptr.is_null() && !p.is_null() {
        // SAFETY: both blocks are live and at least this long; the old one
        // came from here with `osize`, and is kept, poisoned, as a freed one
        unsafe {
            std::ptr::copy_nonoverlapping(ptr, p, osize.min(nsize));
            ptr.write_bytes(POISON, osize);
        }
        held.0.borrow_mut().push((ptr, osize));
    }
    p as *mut c_void
}

/// `source`'s result on a Vm of `version` and hash seed `seed` with every
/// library open, the trace JIT recording after `hot` back edges (none when
/// `hot` is 0), its memory never reused.
pub(crate) fn run(version: LuaVersion, seed: u32, hot: u32, source: &str) -> String {
    run_with(version, seed, hot, source, |_| {})
}

/// [`run`] with `setup` given the Vm before `source` runs.
pub(crate) fn run_with(
    version: LuaVersion,
    seed: u32,
    hot: u32,
    source: &str,
    setup: impl FnOnce(&mut Vm),
) -> String {
    let held = Box::into_raw(Box::new(Held::default()));
    // SAFETY: `quarantine` follows the `lua_Alloc` contract and `held`
    // outlives the Vm, dropped below before it
    let mem = unsafe { MemOwner::raw(quarantine, held as *mut c_void, version) };
    let mut vm = Vm::try_new_minimal_with_mem_and_seed(version, mem, seed).expect("a Vm");
    luna_jit::install_default_jit(&mut vm);
    vm.open_all_libs().expect("the libraries open");
    vm.set_jit_enabled(false);
    if hot == 0 {
        vm.set_trace_jit_enabled(false);
    } else {
        vm.jit_mut().trace_hot_threshold = hot;
        vm.jit_mut().call_hot_threshold = hot;
        vm.set_trace_tier(TraceTier::Auto);
        vm.set_trace_tier_up_at(37);
    }
    setup(&mut vm);
    watch_panics();
    let panics = panics_seen();
    let out = match vm.eval(source) {
        Ok(r) => match r.first() {
            Some(Value::Int(i)) => i.to_string(),
            // 5.1's numbers are all floats
            Some(Value::Float(f)) if f.fract() == 0.0 => (*f as i64).to_string(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_display(&e)),
    };
    // a native's panic, which a `pcall` may have taken as an error
    assert_eq!(panics_seen(), panics, "a panic in the run");
    drop(vm);
    // SAFETY: made above; the Vm, its only user, is gone
    let held = unsafe { Box::from_raw(held) };
    for (p, n) in held.0.take() {
        // SAFETY: each block came from `quarantine` with this size
        unsafe { dealloc(p, Layout::from_size_align_unchecked(n, ALIGN)) };
    }
    out
}
