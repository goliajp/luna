//! v2.2 UAF-A.2 — ASAN repro fixture for the Lua55 sort.lua AA
//! load+collectgarbage SIGSEGV first surfaced in v2.1 CI run
//! 28300507264. Tracked in `.dev/known-bugs/sort-aa-load-
//! collectgarbage-segv.md`. Run under the luna-asan docker image
//! to capture the actual UAF site instead of the downstream
//! Vec-metadata sentinel panic.
//!
//! Local:    cargo test --release -p luna-core --test v2_2_uaf_a_sort_aa_asan -- --nocapture
//! ASAN:     bash .dev/asan-docker/run-asan.sh v2_2_uaf_a_sort_aa_asan
//!
//! macOS / Apple-malloc PASSES this test; Linux glibc + Windows
//! allocator SIGSEGV after the sorts and the AA killer fire.
//! The file `.dev/rfcs/v2.2-uaf-a-asan-trace.md` captures the ASAN
//! trace + the hypothesis the fix is built from.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// v2.3 P1B-D: the cfg-gated `#[ignore]` is gone — the underlying
/// UAF-A/C is closed by `finish_results` slot-clear discipline in
/// v2.3. Runs unconditionally on all platforms now.
/// See `.dev/known-bugs/fixed/sort-aa-load-collectgarbage-segv-uaf-a.md`.
#[test]
fn sort_lua_full_file_under_assert_wrapper() {
    const PREAMBLE: &[u8] = b"do _G.__luna_assert_total=0 _G.__luna_assert_hit=0 _G.assert=function(v,msg,...) _G.__luna_assert_total=_G.__luna_assert_total+1 if v then _G.__luna_assert_hit=_G.__luna_assert_hit+1 return v,msg,... end if msg==nil then msg='assertion failed!' end error(msg,2) end end ";
    let body = std::fs::read(
        std::env::current_dir()
            .unwrap()
            .join("tests/official/lua-5.5.0-tests/sort.lua"),
    )
    .expect("sort.lua at tests/official/lua-5.5.0-tests/sort.lua");
    let mut src = Vec::new();
    src.extend_from_slice(PREAMBLE);
    src.extend_from_slice(&body);
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_global("_U", Value::Bool(true)).unwrap();
    vm.set_memory_cap(Some(1usize << 30));
    vm.eval(std::str::from_utf8(&src).unwrap())
        .expect("sort.lua must complete cleanly under ASAN");
}
