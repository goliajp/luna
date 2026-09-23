//! Bytecode loader + dispatcher: arbitrary bytes are loaded as a binary
//! chunk (luna's own format or PUC 5.1-5.5, through the verifier) and, when
//! the load succeeds, run. A chunk the verifier accepts must not crash the
//! VM; a panic, abort or sanitizer report here is a verifier gap or a VM
//! bug.
//!
//! Seed it with valid chunks (`string.dump` output, `luac` output) so
//! mutations stay near the format.
//!
//! Run:
//!     cargo +nightly fuzz run fuzz_dump_exec

#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const VERSIONS: &[LuaVersion] = &[
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    for &ver in VERSIONS {
        let mut vm = Vm::new(ver);
        vm.set_puc_bytecode_loading(true);
        vm.set_instr_budget(Some(100_000));
        vm.set_memory_cap(Some(64 * 1024 * 1024));
        if let Ok(f) = vm.load(data, b"=fuzz") {
            // both a result and a Lua error are fine outcomes
            let _ = vm.call_value(Value::Closure(f), &[]);
        }
    }
});
