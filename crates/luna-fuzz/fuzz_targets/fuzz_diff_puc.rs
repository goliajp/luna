//! Differential vs PUC fuzz target.
//!
//! Arbitrary-derived side-effect-free Expr → list of `print(expr)`
//! stmts → byte-diff luna vs PUC reference binary (`$PUC_LUA`,
//! default `lua5.5`). Panics on any divergence — that's a luna bug.
//!
//! Skipped (no panic) when `$PUC_LUA` is unset OR the PUC binary
//! fails to spawn — local dev without PUC installed shouldn't fail
//! the fuzz harness; CI's `.github/workflows/fuzz.yml` installs
//! lua5.5 explicitly when matrix target = fuzz_diff_puc.
//!
//! Why this complements the static `diff_puc.rs` integration test:
//! that test ships 5 hand-picked deterministic fixtures. This fuzz
//! target generates infinite distinct programs + catches
//! semantic-divergence bugs the fixed corpus misses.
//!
//! `$LUNA_FUZZ_DIALECT` (`5.1` … `5.5`, default `5.5`) picks the dialect
//! luna runs; point `$PUC_LUA` at the matching interpreter.
//!
//! Run:
//!     PUC_LUA=$(which lua5.5) cd crates/luna-fuzz
//!     cargo +nightly fuzz run fuzz_diff_puc -- -runs=1000

#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_core::runtime::Value;
use luna_core::vm::Vm;

#[path = "program.rs"]
mod program;

use program::{Program, dialect, normalize, render};

fn run_puc(source: &str) -> Option<String> {
    let out = program::run_puc(source.as_bytes())?;
    if !out.status.success() || !out.stderr.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_luna(source: &str) -> Option<String> {
    const PREAMBLE: &str = r#"
_G.__diff_puc_buf = ""
function print(...)
    local t = {}
    local n = select('#', ...)
    for i = 1, n do t[i] = tostring(select(i, ...)) end
    _G.__diff_puc_buf = _G.__diff_puc_buf .. table.concat(t, '\t') .. '\n'
end
"#;
    let mut full = String::with_capacity(PREAMBLE.len() + source.len() + 32);
    full.push_str(PREAMBLE);
    full.push_str(source);
    full.push_str("\nreturn _G.__diff_puc_buf\n");
    let mut vm = Vm::new(dialect());
    vm.set_memory_cap(Some(16 * 1024 * 1024));
    let r = vm.eval(&full).ok()?;
    match r.first() {
        Some(Value::Str(s)) => Some(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        _ => None,
    }
}

fuzz_target!(|p: Program| {
    let source = render(&p);
    let Some(puc) = run_puc(&source) else { return };
    let Some(luna) = run_luna(&source) else {
        panic!(
            "luna eval failed where PUC succeeded\n=== source ===\n{source}\n=== PUC stdout ===\n{puc}"
        );
    };
    let puc_n = normalize(&puc);
    let luna_n = normalize(&luna);
    if puc_n != luna_n {
        panic!(
            "diff_puc: luna ≠ PUC\n=== source ===\n{source}\n=== PUC ===\n{puc_n}\n=== luna ===\n{luna_n}"
        );
    }
});
