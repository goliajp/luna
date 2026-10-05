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

use program::{NanPick, Program, dialect, normalize, render, render_with};

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
    if puc_n != luna_n && !nan_pair_explains(&p, &puc_n, &luna_n) {
        panic!(
            "diff_puc: luna ≠ PUC\n=== source ===\n{source}\n=== PUC ===\n{puc_n}\n=== luna ===\n{luna_n}"
        );
    }
});

/// Whether every line where luna and PUC differ differs only in a NaN's
/// sign, and luna prints PUC's line once `+` and `*` of two NaNs are made
/// to return the first or the second operand. Which one PUC returns comes
/// from how gcc compiled that PUC version, not from Lua (see NanPick), so
/// such a line is not a luna bug. A NaN whose sign is wrong for any other
/// reason stays wrong under every choice and is still reported.
fn nan_pair_explains(p: &Program, puc: &str, luna: &str) -> bool {
    let unsigned = |s: &str| s.replace("-nan", "nan");
    let (puc_lines, luna_lines): (Vec<&str>, Vec<&str>) = (puc.lines().collect(), luna.lines().collect());
    if puc_lines.len() != luna_lines.len() {
        return false;
    }
    let differing: Vec<usize> = (0..puc_lines.len())
        .filter(|&i| puc_lines[i] != luna_lines[i])
        .collect();
    if differing
        .iter()
        .any(|&i| unsigned(puc_lines[i]) != unsigned(luna_lines[i]))
    {
        return false;
    }
    let picks = [NanPick::First, NanPick::Second];
    let variants: Vec<String> = picks
        .iter()
        .flat_map(|&add| picks.iter().map(move |&mul| (add, mul)))
        .filter_map(|(add, mul)| run_luna(&render_with(p, add, mul)).map(|out| normalize(&out)))
        .collect();
    differing.iter().all(|&i| {
        variants
            .iter()
            .any(|v| v.lines().nth(i) == Some(puc_lines[i]))
    })
}
