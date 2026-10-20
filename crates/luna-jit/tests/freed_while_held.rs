//! A trace being recorded holds the prototypes and strings it went
//! through without keeping them alive; a collection that frees one must
//! drop the recording before the sweep, without reading the freed object.
//!
//! The Vms here run on memory that is never reused: a freed block is
//! zeroed and kept until the Vm is gone, so code that reads a freed object
//! finds zeros, the same every run. And the `gc-stress` build checks, at
//! the end of every collection, that a recording still under way holds
//! only objects the collector keeps.

use std::alloc::{Layout, alloc, dealloc};
use std::cell::RefCell;
use std::ffi::c_void;

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
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

/// `source`'s result on a Vm of `version` and hash seed `seed` with every
/// library open, the trace JIT recording after `hot` back edges (none when
/// `hot` is 0), its memory never reused.
fn run(version: LuaVersion, seed: u32, hot: u32, source: &str) -> String {
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
        vm.jit.trace_hot_threshold = hot;
        vm.jit.call_hot_threshold = hot;
        vm.set_trace_tier(TraceTier::Auto);
        vm.set_trace_tier_up_at(37);
    }
    let out = match vm.eval(source) {
        Ok(r) => match r.first() {
            Some(Value::Int(i)) => i.to_string(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_display(&e)),
    };
    drop(vm);
    // SAFETY: made above; the Vm, its only user, is gone
    let held = unsafe { Box::from_raw(held) };
    for (p, n) in held.0.take() {
        // SAFETY: each block came from `quarantine` with this size
        unsafe { dealloc(p, Layout::from_size_align_unchecked(n, ALIGN)) };
    }
    out
}

fn same_as_interp(version: LuaVersion, source: &str) {
    Vm::check_recordings_after_collections(true);
    for seed in 1..=8 {
        let want = run(version, seed, 0, source);
        for hot in [1, 3] {
            assert_eq!(
                run(version, seed, hot, source),
                want,
                "seed {seed}, hot {hot}"
            );
        }
    }
}

/// A loop recorded through a function it loads, drops and then collects in
/// full: the full collection sweeps right after it marks, and the
/// recording holding the dead prototype is dropped before that sweep.
#[test]
fn a_full_collection_during_a_recording_reads_nothing_freed() {
    same_as_interp(
        LuaVersion::Lua54,
        "local n = 0
         for i = 1, 200 do
           local f = load('return ' .. i)
           n = n + f()
           f = nil
           collectgarbage()
         end
         return n",
    );
}

/// The same in 5.1, whose full collection and loader differ.
#[test]
fn a_full_collection_during_a_5_1_recording_reads_nothing_freed() {
    same_as_interp(
        LuaVersion::Lua51,
        "local n = 0
         for i = 1, 200 do
           local f = loadstring('return ' .. i)
           n = n + f()
           f = nil
           collectgarbage()
         end
         return n",
    );
}
