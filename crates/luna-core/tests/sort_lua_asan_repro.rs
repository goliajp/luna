//! ASAN repro fixture for the Lua55 sort.lua
//! load+collectgarbage SIGSEGV. Run under the luna-asan docker image
//! to capture the actual UAF site instead of the downstream
//! Vec-metadata sentinel panic.
//!
//! Local:    cargo test --release -p luna-core --test sort_lua_asan_repro -- --nocapture
//!
//! The bug was allocator-dependent: Apple malloc hid it, while the
//! Linux glibc and Windows allocators SIGSEGV'd after the sorts.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Runs unconditionally on all platforms: the underlying use-after-free
/// is closed by the `finish_results` slot-clear discipline.
#[test]
fn sort_lua_full_file_under_assert_wrapper() {
    const PREAMBLE: &[u8] = b"do _G.__luna_assert_total=0 _G.__luna_assert_hit=0 _G.assert=function(v,msg,...) _G.__luna_assert_total=_G.__luna_assert_total+1 if v then _G.__luna_assert_hit=_G.__luna_assert_hit+1 return v,msg,... end if msg==nil then msg='assertion failed!' end error(msg,2) end end ";
    let body = std::fs::read(
        std::env::current_dir()
            .unwrap()
            .join("tests/official/lua-5.5.1-tests/sort.lua"),
    )
    .expect("sort.lua at tests/official/lua-5.5.1-tests/sort.lua");
    let mut src = Vec::new();
    src.extend_from_slice(PREAMBLE);
    src.extend_from_slice(&body);
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_global("_U", Value::Bool(true)).unwrap();
    vm.set_memory_cap(Some(1usize << 30));
    vm.eval(std::str::from_utf8(&src).unwrap())
        .expect("sort.lua must complete cleanly under ASAN");
}
