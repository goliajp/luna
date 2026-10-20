//! What the runtime holds outside Lua's reach, through collections: a
//! trace being recorded holds the prototypes and strings it went through
//! without keeping them alive, and a collection that frees one must drop
//! the recording before the sweep, without reading the freed object; a
//! coroutine an error killed keeps its frames, whose functions the
//! collector must keep for the traceback and `getinfo` made from them.
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
            // 5.1's numbers are all floats
            Some(Value::Float(f)) if f.fract() == 0.0 => (*f as i64).to_string(),
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

/// A coroutine killed by an error raised in a function a native called:
/// that function is held by nothing but the frames the dead coroutine
/// keeps. Its traceback is the same before and after full collections.
#[test]
fn a_dead_coroutine_keeps_the_functions_of_its_frames() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Lua55] {
        let got = run(
            version,
            1,
            0,
            "local co = coroutine.create(function()
               string.gsub('a', 'a', function() local x = {} error('x') end)
             end)
             coroutine.resume(co)
             local before = debug.traceback(co)
             collectgarbage() collectgarbage()
             local junk = {}
             for i = 1, 2000 do junk[i] = function() return i end end
             local after = debug.traceback(co)
             local info = debug.getinfo(co, 1, 'S')
             return before == after and info ~= nil and 1 or 0",
        );
        assert_eq!(got, "1", "{version:?}");
    }
}


/// `debug.sethook` makes the hook table: the registry key it looked the
/// table up by may be collected by the table's allocation (here, by the
/// `gc-stress` build's collection there), so the key is made again for
/// the store, as `lua_setfield` does. Run in the `luna` command, whose
/// state opens its libraries as `lua.c` does, with glibc's allocator
/// filling what is freed (`MALLOC_PERTURB_`), so a key read after it was
/// freed is not the key.
#[test]
fn the_hook_table_key_outlives_the_table_allocation() {
    let src = "local n = 0
        local h = function() n = n + 1 end
        debug.sethook(h, 'c')
        local f = function() end
        f(); f()
        local got = debug.gethook()
        debug.sethook()
        print(n > 0 and got == h)";
    for version in ["5.2", "5.4", "5.5"] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_luna"))
            .args([&format!("--lua={version}"), "-e", src])
            .env("MALLOC_PERTURB_", "165")
            .output()
            .expect("the luna command runs");
        assert_eq!(
            (out.status.success(), String::from_utf8_lossy(&out.stdout).trim()),
            (true, "true"),
            "{version}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The open upvalue a coroutine's frame left without a closure, taken off
/// the thread's list when the marking ends, no longer names the thread,
/// which the same sweep may free (the `gc-verify` build checks that no
/// object the sweep keeps holds one it frees).
#[cfg(feature = "gc-verify")]
#[test]
fn a_dead_open_upvalue_names_no_thread() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
        let got = run(
            version,
            1,
            0,
            "collectgarbage('restart')
             local keep, n = nil, 0
             for i = 1, 20 do
               local co = coroutine.create(function()
                 local d = i
                 keep = function() return d end
                 local lost = function() d = d + 1 end
                 lost = nil
                 coroutine.yield()
               end)
               coroutine.resume(co)
               co = nil
               collectgarbage()
               n = n + keep()
             end
             return n",
        );
        assert_eq!(got, "210", "{version:?}");
    }
}
