//! v2.2 Phase 5 (DP) — luna-vs-PUC differential test harness.
//!
//! For each fixture under `tests/diff_puc/*.lua`, run the source
//! on PUC's reference `$PUC_LUA` binary (defaulting to `lua5.5`)
//! and on luna's in-process `Vm::eval`, then compare stdout
//! byte-for-byte. Any divergence is a luna bug to file.
//!
//! Acceptance: 5 deterministic fixtures (no `math.random` /
//! `os.time` / `io` non-determinism) ship in v2.2; v2.3+ expands
//! to fuzz-driven inputs + the full PUC official suite.
//!
//! Env:
//! - `PUC_LUA` (optional) — path to the PUC binary. Defaults to
//!   `lua5.5` (PATH lookup). Set in CI by `.github/workflows/
//!   diff-puc.yml`'s install step.
//! - `LUNA_DIFF_PUC_VERBOSE=1` (optional) — print both outputs
//!   on every fixture even on success.
//!
//! Local run:
//!     PUC_LUA=$(which lua5.5) cargo test --release \
//!         -p luna-core --test diff_puc -- --nocapture

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/diff_puc")
}

fn list_fixtures() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let dir = fixture_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("diff_puc fixture dir {} missing: {e}", dir.display()));
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("lua") {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Run `source` via PUC's reference binary and capture stdout.
/// Only a **missing binary** skips (dev machines without
/// `lua5.5` shouldn't fail the suite; CI installs it
/// explicitly). If PUC runs but errors (non-zero exit or
/// non-empty stderr), the fixture itself is broken and the test
/// **fails** — silently skipping here shipped fixtures that
/// never actually diffed (found in v2.12: 5 of the first 150).
fn run_on_puc(path: &Path, source: &str) -> Option<String> {
    let bin = std::env::var("PUC_LUA").unwrap_or_else(|_| "lua5.5".to_string());
    let mut child = match Command::new(&bin)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            eprintln!("[diff_puc] PUC binary `{bin}` not found; skipping");
            return None;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(source.as_bytes());
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[diff_puc] PUC wait failed: {e}");
            return None;
        }
    };
    if !out.status.success() || !out.stderr.is_empty() {
        panic!(
            "[diff_puc] fixture {} errors on PUC itself (status={:?} stderr={}) — fix the fixture",
            path.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `source` via luna's in-process `Vm::eval` with stdout
/// redirection. luna's `print` builtin writes to the host
/// process's stdout via `println!`, so capture the harness's own
/// stdout by spawning a subprocess that exec's `cargo run` —
/// expensive. Instead, swap luna's print impl during the run
/// (via `Vm::set_print_handler` if available) or capture from a
/// thread-local buffer. For v2.2 simplicity: rebuild Lua source
/// with a leading `local _print = print; function print(...)
/// local t = {} for i = 1, select('#', ...) do t[i] = tostring(
/// select(i, ...)) end _G.__luna_diff_puc_buf = _G.
/// __luna_diff_puc_buf .. table.concat(t, '\t') .. '\n' end`
/// preamble that redirects to a global buffer, then read it
/// back at the end.
fn run_on_luna(source: &str) -> String {
    const PREAMBLE: &str = r#"
_G.__luna_diff_puc_buf = ""
local _orig_print = print
function print(...)
    local t = {}
    local n = select('#', ...)
    for i = 1, n do t[i] = tostring(select(i, ...)) end
    _G.__luna_diff_puc_buf = _G.__luna_diff_puc_buf .. table.concat(t, '\t') .. '\n'
end
local _orig_io_write = io.write
io.write = function(...)
    local t = {}
    local n = select('#', ...)
    for i = 1, n do t[i] = tostring(select(i, ...)) end
    _G.__luna_diff_puc_buf = _G.__luna_diff_puc_buf .. table.concat(t)
end
"#;
    const POSTAMBLE: &str = "\nreturn _G.__luna_diff_puc_buf\n";

    let mut full = String::with_capacity(PREAMBLE.len() + source.len() + POSTAMBLE.len());
    full.push_str(PREAMBLE);
    full.push_str(source);
    full.push_str(POSTAMBLE);

    let mut vm = Vm::new(LuaVersion::Lua55);
    let r = vm
        .eval(&full)
        .expect("luna eval must not error on diff fixtures");
    match r.first() {
        Some(luna_core::runtime::Value::Str(s)) => {
            String::from_utf8_lossy(s.as_bytes()).into_owned()
        }
        other => panic!("expected diff_puc buffer string from luna; got {other:?}"),
    }
}

/// Normalize whitespace runs + trim trailing newlines so trivial
/// formatting drift (e.g. `\r\n` vs `\n`, double-newline at EOF)
/// doesn't fail the diff. Semantic content stays.
fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end_matches('\n').to_string()
}

fn diff_one(path: &Path) {
    let source =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let puc = match run_on_puc(path, &source) {
        Some(o) => o,
        None => {
            eprintln!(
                "[diff_puc] skip {} (PUC binary unavailable)",
                path.display()
            );
            return;
        }
    };
    let luna = run_on_luna(&source);
    let puc_n = normalize(&puc);
    let luna_n = normalize(&luna);
    if std::env::var_os("LUNA_DIFF_PUC_VERBOSE").is_some() {
        eprintln!("[diff_puc] {} PUC:\n{puc_n}", path.display());
        eprintln!("[diff_puc] {} luna:\n{luna_n}", path.display());
    }
    if puc_n != luna_n {
        eprintln!("=== PUC output ({}) ===\n{puc_n}", path.display());
        eprintln!("=== luna output ({}) ===\n{luna_n}", path.display());
        panic!("diff: {} diverged between luna and PUC", path.display());
    }
}

#[test]
fn diff_against_puc_5_5() {
    let fixtures = list_fixtures();
    assert!(
        !fixtures.is_empty(),
        "no diff_puc fixtures found under tests/diff_puc/"
    );
    eprintln!("[diff_puc] running {} fixtures", fixtures.len());
    for f in fixtures {
        diff_one(&f);
    }
}
