//! v2.0 Track CV fuzz harness — VM dispatcher (eval pipeline end-to-end).
//!
//! Feeds arbitrary bytes as Lua source into `Vm::eval` with an instr
//! budget cap. Exercises parser + compiler + dispatcher in one path.
//! Per-track content fill will add a `puc-bytecode + Vm::load` variant
//! that hits the dispatcher with adversarial bytecode (different attack
//! surface from source — compiler-validated invariants are bypassed).
//!
//! The instr budget cap exists so the fuzzer can't be wedged by
//! infinite loops produced from valid-looking inputs (a real bug class
//! we want to catch is `instr_budget never trips` — but at fuzzer
//! granularity we cap conservatively to keep iteration throughput up).
//!
//! Run:
//!     cargo +nightly fuzz run fuzz_vm_dispatch

#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fuzz_target!(|data: &[u8]| {
    // 16 KiB cap: source longer than this rarely surfaces new bugs and
    // dominates wall-clock per iteration.
    if data.len() > 16 * 1024 {
        return;
    }
    // Only accept input that's UTF-8: the lexer treats source as
    // bytes, but the str API on the Vm side wants &str. Non-UTF-8
    // fuzzing is covered by fuzz_parser (parser takes &[u8]).
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua55);
    // Hard instr cap so the fuzzer survives infinite-loop inputs.
    vm.set_instr_budget(Some(100_000));
    // Memory cap so allocator-OOM doesn't kill the libfuzzer process.
    vm.set_memory_cap(Some(64 * 1024 * 1024));
    // eval returns Result<Vec<Value>, LuaError>; both arms are valid
    // outcomes. A panic / abort / true OOM is the real bug.
    let _ = vm.eval(src);
});
