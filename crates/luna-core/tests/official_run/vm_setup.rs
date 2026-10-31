//! The Vm a test file runs in, set up as lua.c sets up its state.

use super::*;

/// The memory the stress files may use.
pub(super) const STRESS_CAP: usize = 1 << 30;

/// A fresh Vm for the file, as lua.c runs it. heavy.lua's stress loops run
/// into the end of memory and catch "not enough memory" as under PUC's
/// test allocator: the Vm's allocations go through a 1 GiB limit.
pub(super) fn new_vm(version: LuaVersion, label: &str) -> Vm {
    let mut vm = if label == "heavy.lua" {
        let limit = luna_core::runtime::mem::MemoryLimit(STRESS_CAP);
        Vm::new_with_mem(
            version,
            luna_core::runtime::mem::MemOwner::policy(Box::new(limit)),
        )
    } else {
        Vm::new(version)
    };
    generational_as_lua_c(&mut vm, version);
    vm
}

/// lua.c from 5.4: `lua_gc(L, LUA_GCRESTART)`, `lua_gc(L, LUA_GCGEN)`.
pub(super) fn generational_as_lua_c(vm: &mut Vm, version: LuaVersion) {
    use luna_core::__private::VmCApiLoad;
    let op = match version {
        LuaVersion::Lua55 => 7,
        v if v >= LuaVersion::Lua54 => 10,
        _ => return,
    };
    vm.host_gc(1, [0; 3]).expect("restart");
    vm.host_gc(op, [0; 3]).expect("generational");
}

/// The `_U` / `_port` / `_soft` / `_noposix` globals the file runs under.
/// Memory cap and the `_U` / `_port` / `_soft` / `_noposix` globals the
/// file runs under.
pub(super) fn configure_vm(vm: &mut Vm, label: &str) {
    // a runtime memory cap for the other stress files PUC's outer driver
    // gates behind a host wall-clock budget (heavy.lua's limit is the
    // allocation context's, see `new_vm`). For verybig/memerr/sort the cap
    // is pure headroom: none of them push net live bytes anywhere near
    // 1 GiB (verybig has `_soft=true` set below, memerr early-returns when
    // `T` is nil, sort's working set is ~50k Values ≈ 1.2 MB)
    if matches!(label, "verybig.lua" | "memerr.lua" | "sort.lua") {
        vm.set_memory_cap(Some(STRESS_CAP));
    }
    vm.set_global("_U", Value::Bool(true)).unwrap();
    // attrib.lua's lines 79-356 exercise dynamic C-library loading
    // (`package.loadlib`) which luna does not ship; `_port=true` is the
    // PUC-sanctioned escape hatch for non-portable subsections.
    if label == "attrib.lua" {
        vm.set_global("_port", Value::Bool(true)).unwrap();
    }
    // main.lua exercises the standalone-interpreter command line
    // (`os.execute`, `arg[-N]` for the binary name, tmpfile-based
    // sub-invocations). Inside the gate harness there is no real
    // interpreter binary to dispatch back into, so set `_port=true`
    // and let the `if _port then return end` at top exit cleanly.
    if label == "main.lua" {
        vm.set_global("_port", Value::Bool(true)).unwrap();
    }
    // big.lua / verybig.lua's `if _soft then return … end` short-circuits
    // the multi-megabyte-prog / 70k-line-prog generation that PUC's
    // outer driver (all.lua) gates behind a wall-clock budget. The gate
    // harness honours the same escape hatch: still verifies the early
    // assertions (table-construction round-trip, RK boundary cases) but
    // skips the synthesized-program section that depends on either a
    // top-level `coroutine.yield` driver (big.lua) or platform-tunable
    // limits (verybig.lua).
    if label == "big.lua" || label == "verybig.lua" {
        vm.set_global("_soft", Value::Bool(true)).unwrap();
    }
    // files.lua's `if not _port` block runs popen/execute/`io.tmpfile`
    // off the `arg` global (which luna does not populate from a host
    // command line) — that's the PUC-sanctioned non-portable subsection.
    // The earlier and later blocks (i/o behaviour, date/time, loadfile)
    // still run. 5.2 / 5.3 use `_noposix` (not `_port`) for the same
    // popen/`os.execute` block, so set both for cross-dialect coverage.
    if label == "files.lua" || label == "files_win.lua" {
        vm.set_global("_port", Value::Bool(true)).unwrap();
        vm.set_global("_noposix", Value::Bool(true)).unwrap();
    }
}
