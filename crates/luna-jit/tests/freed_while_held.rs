//! What the runtime holds outside Lua's reach, through collections: a
//! trace being recorded holds the prototypes and strings it went through
//! without keeping them alive, and a collection that frees one must drop
//! the recording before the sweep, without reading the freed object; a
//! coroutine an error killed keeps its frames, whose functions the
//! collector must keep for the traceback and `getinfo` made from them.
//!
//! The Vms here run on memory that is never reused: a freed block (and the
//! old block of a reallocation) is filled with a poison byte and kept until
//! the Vm is gone, so code that reads a freed object finds lengths and
//! pointers that are plainly wrong, the same every run, and most likely
//! fails on them at once. And the `gc-stress` build checks, at the end of
//! every collection, that a recording still under way holds only objects
//! the collector keeps.

#[path = "freed_while_held/harness.rs"]
mod harness;

use harness::{run, run_with};
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

fn same_as_interp(version: LuaVersion, source: &str) {
    luna_core::__private::check_recordings_after_collections(true);
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
            .env("MALLOC_PERTURB_", "219")
            .output()
            .expect("the luna command runs");
        assert_eq!(
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).trim()
            ),
            (true, "true"),
            "{version}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// The same in the Vms here: the key the table was looked up by is freed
/// by the table's allocation, and the table must still be found under its
/// name.
#[test]
fn the_hook_table_is_under_its_key() {
    for (version, key) in [
        (LuaVersion::Lua52, "YEKH_"),
        (LuaVersion::Lua54, "YEKKOOH_"),
        (LuaVersion::Lua55, "YEKKOOH_"),
    ] {
        let src = format!(
            "collectgarbage('restart')
             debug.sethook(function() end, 'c')
             -- the key's name made only now: a constant of the chunk would
             -- keep the key alive
             local found = type(debug.getregistry()[('{key}'):reverse()]) == 'table'
             debug.sethook()
             return found and 1 or 0"
        );
        assert_eq!(run(version, 1, 0, &src), "1", "{version:?}");
    }
}

/// The open upvalue a coroutine's frame left without a closure, taken off
/// the thread's list when the marking ends, no longer names the thread,
/// which the same sweep may free (the `gc-verify` build checks that no
/// object the sweep keeps holds one it frees).
/// `debug.gethook` of a hook set from outside Lua makes "external hook"
/// and then the mask: in PUC a collection at the second keeps the first,
/// which is on the C function's stack.
#[test]
fn gethook_keeps_the_hook_name_through_the_mask_push() {
    use luna_core::vm::exec::{HOOK_MASK_CALL, RustHookEvent};
    // the script names the string in capitals: a constant would keep it
    let source =
        "local h, m = debug.gethook() return h:upper() == 'EXTERNAL HOOK' and m == 'c' and 1";
    for v in [LuaVersion::Lua51, LuaVersion::Lua53, LuaVersion::Lua55] {
        let quiet: fn(&mut Vm, RustHookEvent) = |_, _| {};
        let got = run_with(v, 1, 0, source, |vm| {
            vm.set_rust_debug_hook(Some(quiet), HOOK_MASK_CALL, 0)
        });
        assert_eq!(got, "1", "{v:?}");
    }
}

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
