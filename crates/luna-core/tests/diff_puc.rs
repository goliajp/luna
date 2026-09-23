//! luna-vs-PUC differential test harness (v2.2 origin; v2.14
//! multi-dialect + multi-channel).
//!
//! ## Layout & dialects
//! Fixtures live under `tests/diff_puc/5.x/` — each subtree runs
//! against `LuaVersion::Lua5x` on the luna side and the `PUC_LUA_5X`
//! interpreter on the PUC side (`PUC_LUA` is the 5.5 fallback).
//! The GROUND TRUTH is each version's DEFAULT `make` build,
//! including its compat defaults (5.2/5.3 ship -DLUA_COMPAT_*, so
//! e.g. `loadstring` exists on 5.2 and `bit32` on 5.3).
//!
//! ## Channels
//! - stdout mode (default): the chunk must SUCCEED on both sides
//!   (a PUC-side error fails the run — no silent skips) and stdout
//!   must match byte-for-byte after newline normalization. This
//!   pins the exit-code-0 leg implicitly.
//! - error mode (`*_err.lua`): the chunk must FAIL at top level on
//!   both sides (PUC: non-zero exit; luna: eval Err) and the error
//!   text must match after position-prefix normalization — the
//!   `<chunk>:<line>:` prefix is stripped (chunknames differ and
//!   the capture preamble shifts lines) but its PRESENCE must
//!   agree. stdout is not compared in this mode.
//!
//! ## Env
//! - `PUC_LUA_51` … `PUC_LUA_55` — per-dialect interpreter paths
//!   (CI builds all five from lua.org tarballs; `make posix` for
//!   ≤5.4, `make linux` for 5.5).
//! - `PUC_LUA` — 5.5 fallback for local dev (PATH `lua5.5`).
//! - `LUNA_DIFF_PUC_VERBOSE=1` — print both outputs on success.
//! - `PUC_LUAC_51` … `PUC_LUAC_55` — per-dialect `luac` paths. When set,
//!   `diff_puc_bytecode` also compiles every stdout-mode fixture with that
//!   dialect's `luac`, runs the `.luac` on PUC and loads the same bytes
//!   into luna (same dialect, PUC bytecode loading on): the corpus doubles
//!   as a test of the PUC chunk translators.
//!
//! Local dev: `brew install lua@5.4` (+ `lua` for 5.5); 5.1.5 /
//! 5.2.4 / 5.3.6 build in one `make macosx` each from the lua.org
//! tarballs — point the env vars at `src/lua`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/diff_puc")
}

/// v2.14 HD — dialect directories. Each `tests/diff_puc/5.x/`
/// subtree runs against the matching `LuaVersion` on the luna side
/// and the `PUC_LUA_5X` binary on the PUC side (`PUC_LUA` is the
/// 5.5 fallback for local dev). A dialect whose binary is absent
/// is skipped WITH a per-dialect notice — CI installs all five, so
/// a skip there is an install failure surfaced by the workflow's
/// version-print step.
const DIALECTS: &[(&str, LuaVersion, &str)] = &[
    ("5.1", LuaVersion::Lua51, "PUC_LUA_51"),
    ("5.2", LuaVersion::Lua52, "PUC_LUA_52"),
    ("5.3", LuaVersion::Lua53, "PUC_LUA_53"),
    ("5.4", LuaVersion::Lua54, "PUC_LUA_54"),
    ("5.5", LuaVersion::Lua55, "PUC_LUA_55"),
];

fn puc_bin_for(dialect: &str, env_key: &str) -> Option<String> {
    if let Ok(b) = std::env::var(env_key) {
        return Some(b);
    }
    if dialect == "5.5" {
        return Some(std::env::var("PUC_LUA").unwrap_or_else(|_| "lua5.5".to_string()));
    }
    None
}

fn list_fixtures(dialect_dir: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let dir = fixture_dir().join(dialect_dir);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return out, // dialect dir absent = no fixtures yet
    };
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
fn run_on_puc(path: &Path, bin: &str, source: &str) -> Option<String> {
    let mut child = match Command::new(bin)
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
fn run_on_luna(version: LuaVersion, source: &str) -> String {
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

    let mut vm = Vm::new(version);
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

/// v2.14 HC — error-channel normalization. Splits a Lua error
/// message into (had_position_prefix, message): a leading
/// `<chunk>:<line>: ` is stripped because chunknames differ by
/// design (PUC reads stdin → "stdin"; luna evals → "eval") and the
/// luna side's capture preamble shifts line numbers. WHETHER a
/// position prefix exists is still compared — a fixture whose error
/// is prefixed on one side and bare on the other is a real
/// divergence (see v2.13's resume_error / luaL_where fixes).
fn normalize_err(text: &str) -> (bool, String) {
    let t = text.trim_end();
    if let Some(colon2) = t.find(": ") {
        let head = &t[..colon2];
        if let Some(colon1) = head.rfind(':') {
            let (chunk, line) = head.split_at(colon1);
            if !chunk.is_empty() && line[1..].bytes().all(|c| c.is_ascii_digit()) {
                return (true, t[colon2 + 2..].to_string());
            }
        }
    }
    (false, t.to_string())
}

/// v2.14 HC — `_err.lua` fixtures pin the ERROR channel: the chunk
/// must fail at top level on BOTH interpreters (PUC: non-zero exit;
/// luna: `eval` Err — the exit-code channel), and the normalized
/// error text must agree. stdout emitted before the error is NOT
/// compared in this mode.
fn diff_one_err(path: &Path, bin: &str, version: LuaVersion, source: &str) {
    let mut child = match Command::new(bin)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => {
            eprintln!(
                "[diff_puc] skip {} (PUC binary unavailable)",
                path.display()
            );
            return;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(source.as_bytes());
    }
    let out = child.wait_with_output().expect("PUC wait");
    if out.status.success() {
        panic!(
            "[diff_puc] {}: expected a top-level error on PUC (fixture is _err) \
             but it exited 0",
            path.display()
        );
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first = stderr.lines().next().unwrap_or("");
    // PUC's standalone prints `<progname>: <chunk>:<line>: msg` —
    // progname is argv[0] (an arbitrary path), so strip everything
    // up to the first ": ".
    let puc_msg = first
        .split_once(": ")
        .map(|(_, rest)| rest)
        .unwrap_or(first);
    let (puc_pos, puc_n) = normalize_err(puc_msg);

    let mut vm = Vm::new(version);
    let luna_err = match vm.eval(source) {
        Ok(_) => panic!(
            "[diff_puc] {}: expected a top-level error on luna (fixture is _err) \
             but eval returned Ok",
            path.display()
        ),
        Err(e) => vm.error_display(&e),
    };
    let (luna_pos, luna_n) = normalize_err(&luna_err);

    if (puc_pos, &puc_n) != (luna_pos, &luna_n) {
        eprintln!(
            "=== PUC error   ({}): pos={puc_pos} {puc_n}",
            path.display()
        );
        eprintln!(
            "=== luna error  ({}): pos={luna_pos} {luna_n}",
            path.display()
        );
        panic!(
            "diff: {} error channel diverged between luna and PUC",
            path.display()
        );
    }
}

fn diff_one(path: &Path, bin: &str, version: LuaVersion) {
    let source =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    if path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with("_err"))
    {
        return diff_one_err(path, bin, version, &source);
    }
    let puc = match run_on_puc(path, bin, &source) {
        Some(o) => o,
        None => {
            eprintln!(
                "[diff_puc] skip {} (PUC binary unavailable)",
                path.display()
            );
            return;
        }
    };
    let luna = run_on_luna(version, &source);
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
fn diff_against_puc() {
    let mut total = 0usize;
    for &(dialect, version, env_key) in DIALECTS {
        let fixtures = list_fixtures(dialect);
        if fixtures.is_empty() {
            continue;
        }
        let Some(bin) = puc_bin_for(dialect, env_key) else {
            eprintln!(
                "[diff_puc] dialect {dialect}: {} fixtures SKIPPED — {env_key} not set",
                fixtures.len()
            );
            continue;
        };
        eprintln!(
            "[diff_puc] dialect {dialect}: running {} fixtures against {bin}",
            fixtures.len()
        );
        for f in &fixtures {
            diff_one(f, &bin, version);
        }
        total += fixtures.len();
    }
    assert!(
        total > 0,
        "no diff_puc fixtures ran (all dialects empty/skipped)"
    );
}

const LUAC_ENV: &[(&str, &str)] = &[
    ("5.1", "PUC_LUAC_51"),
    ("5.2", "PUC_LUAC_52"),
    ("5.3", "PUC_LUAC_53"),
    ("5.4", "PUC_LUAC_54"),
    ("5.5", "PUC_LUAC_55"),
];

/// Compile `path` with PUC's `luac` into a file next to the system temp dir.
fn compile_with_luac(luac: &str, path: &Path) -> PathBuf {
    let out = std::env::temp_dir().join(format!(
        "luna-diff-puc-{}-{}.luac",
        std::process::id(),
        path.file_stem()
            .and_then(|s| s.to_str())
            .expect("fixture file name")
    ));
    let st = Command::new(luac)
        .arg("-o")
        .arg(&out)
        .arg(path)
        .status()
        .unwrap_or_else(|e| panic!("[diff_puc] cannot run luac `{luac}`: {e}"));
    assert!(st.success(), "[diff_puc] luac failed on {}", path.display());
    out
}

/// Run a precompiled chunk on PUC; the fixture must succeed there.
fn run_luac_on_puc(path: &Path, bin: &str, luac_file: &Path) -> String {
    let out = Command::new(bin)
        .arg(luac_file)
        .output()
        .unwrap_or_else(|e| panic!("[diff_puc] cannot run `{bin}`: {e}"));
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "[diff_puc] {} (as bytecode) errors on PUC itself: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Load PUC bytecode into luna of the same dialect and run it with stdout
/// captured the way `run_on_luna` does.
fn run_luac_on_luna(path: &Path, version: LuaVersion, bytes: &[u8]) -> String {
    let mut vm = Vm::new(version);
    vm.set_puc_bytecode_loading(true);
    vm.eval(
        r#"
_G.__luna_diff_puc_buf = ""
function print(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__luna_diff_puc_buf = _G.__luna_diff_puc_buf .. table.concat(t, '\t') .. '\n'
end
io.write = function(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__luna_diff_puc_buf = _G.__luna_diff_puc_buf .. table.concat(t)
end
"#,
    )
    .expect("capture preamble");
    let f = vm.load(bytes, b"=luac").unwrap_or_else(|e| {
        panic!(
            "[diff_puc] luna rejects PUC bytecode of {}: {e:?}",
            path.display()
        )
    });
    if let Err(e) = vm.call_value(luna_core::runtime::Value::Closure(f), &[]) {
        let msg = match e.0 {
            luna_core::runtime::Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            v => format!("{v:?}"),
        };
        panic!(
            "[diff_puc] {} (as bytecode) errors on luna: {msg}",
            path.display()
        );
    }
    match vm
        .eval("return _G.__luna_diff_puc_buf")
        .expect("buffer")
        .first()
    {
        Some(luna_core::runtime::Value::Str(s)) => {
            String::from_utf8_lossy(s.as_bytes()).into_owned()
        }
        other => panic!("expected diff_puc buffer string from luna; got {other:?}"),
    }
}

#[test]
fn diff_puc_bytecode() {
    let mut total = 0usize;
    let mut report = Vec::new();
    for &(dialect, version, env_key) in DIALECTS {
        let luac_key = LUAC_ENV
            .iter()
            .find(|(d, _)| *d == dialect)
            .map(|(_, k)| *k)
            .expect("every dialect has a luac env key");
        let (Ok(luac), Some(bin)) = (std::env::var(luac_key), puc_bin_for(dialect, env_key)) else {
            eprintln!("[diff_puc] bytecode {dialect}: SKIPPED — {luac_key} or {env_key} not set");
            continue;
        };
        let fixtures: Vec<PathBuf> = list_fixtures(dialect)
            .into_iter()
            .filter(|p| {
                !p.file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.ends_with("_err"))
            })
            .collect();
        eprintln!(
            "[diff_puc] bytecode {dialect}: running {} fixtures via {luac}",
            fixtures.len()
        );
        let mut failed = Vec::new();
        for f in &fixtures {
            let luac_file = compile_with_luac(&luac, f);
            let bytes = std::fs::read(&luac_file).expect("read luac output");
            let puc = normalize(&run_luac_on_puc(f, &bin, &luac_file));
            let _ = std::fs::remove_file(&luac_file); // temp file; nothing depends on its removal
            let luna = std::panic::catch_unwind(|| run_luac_on_luna(f, version, &bytes));
            match luna {
                Ok(out) if normalize(&out) == puc => {}
                Ok(out) => failed.push(format!(
                    "{}: output differs\n--- PUC ---\n{puc}\n--- luna ---\n{}",
                    f.display(),
                    normalize(&out)
                )),
                Err(e) => failed.push(format!(
                    "{}: {}",
                    f.display(),
                    e.downcast_ref::<String>().cloned().unwrap_or_default()
                )),
            }
        }
        total += fixtures.len();
        if !failed.is_empty() {
            report.push(format!(
                "[diff_puc] bytecode {dialect}: {} of {} fixtures fail:\n{}",
                failed.len(),
                fixtures.len(),
                failed.join("\n")
            ));
        }
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
    if std::env::vars().any(|(k, _)| k.starts_with("PUC_LUAC_")) {
        assert!(total > 0, "PUC_LUAC_* set but no bytecode fixture ran");
    }
}
