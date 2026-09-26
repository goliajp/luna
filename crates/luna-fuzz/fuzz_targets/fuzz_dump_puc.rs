//! Differential target for PUC-format `string.dump`: luna compiles a
//! generated program and dumps it, the stock PUC interpreter of the same
//! dialect (`$PUC_LUA`) loads and runs the dump, and its output must match
//! PUC running the source. Skipped when `$PUC_LUA` is unset or PUC itself
//! rejects the source; `$LUNA_FUZZ_DIALECT` picks the dialect as in
//! `fuzz_diff_puc`.
//!
//! Run:
//!     PUC_LUA=$(which lua5.5) cargo +nightly fuzz run fuzz_dump_puc --fuzz-dir .

#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_core::runtime::Value;
use luna_core::vm::Vm;

#[path = "program.rs"]
mod program;

use program::{Program, dialect, normalize, render};

fn luna_dump(source: &str) -> Result<Vec<u8>, String> {
    let mut vm = Vm::new(dialect());
    vm.set_memory_cap(Some(16 * 1024 * 1024));
    let code = format!("return string.dump(assert((loadstring or load)([======[{source}]======])))");
    match vm.eval(&code) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => Ok(s.as_bytes().to_vec()),
            other => Err(format!("string.dump returned {other:?}")),
        },
        Err(e) => Err(e.to_string()),
    }
}

fuzz_target!(|p: Program| {
    let source = render(&p);
    let Some(from_source) = program::run_puc(source.as_bytes()) else { return };
    if !from_source.status.success() || !from_source.stderr.is_empty() {
        return;
    }
    let dump = match luna_dump(&source) {
        Ok(d) => d,
        Err(e) => panic!("luna could not dump a program PUC runs\n=== source ===\n{source}\n=== error ===\n{e}"),
    };
    let Some(from_dump) = program::run_puc(&dump) else { return };
    let stderr = String::from_utf8_lossy(&from_dump.stderr);
    if !from_dump.status.success() || !stderr.is_empty() {
        panic!("PUC rejected luna's dump\n=== source ===\n{source}\n=== PUC stderr ===\n{stderr}");
    }
    let want = normalize(&String::from_utf8_lossy(&from_source.stdout));
    let got = normalize(&String::from_utf8_lossy(&from_dump.stdout));
    if want != got {
        panic!("dump run ≠ source run\n=== source ===\n{source}\n=== PUC on source ===\n{want}\n=== PUC on luna's dump ===\n{got}");
    }
});
