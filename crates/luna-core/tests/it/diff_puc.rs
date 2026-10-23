//! luna-vs-PUC differential test harness.
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
//! - error mode (`*_err.lua`): the chunk must FAIL at top level on both
//!   sides and the error text match with the `<chunk>:<line>:` prefix
//!   stripped (its presence must agree); stdout is not compared.
//!
//! ## Env
//! - `PUC_LUA_51` … `PUC_LUA_55` — per-dialect interpreter paths
//!   (CI builds all five from lua.org tarballs; `make posix` for
//!   ≤5.4, `make linux` for 5.5).
//! - `PUC_LUA` — 5.5 fallback for local dev (PATH `lua5.5`).
//! - PUC runs with `TZ=UTC`: luna has no time zones, so local time is UTC.
//! - `LUNA_DIFF_PUC_VERBOSE=1` — print both outputs on success.
//! - `PUC_LUAC_51` … `PUC_LUAC_55` — per-dialect `luac` paths. With the
//!   matching `PUC_LUA_5X`, `diff_puc_bytecode` compiles every fixture of
//!   the dialect with that `luac`, runs the `.luac` on PUC and loads the
//!   same bytes into luna (same dialect, PUC bytecode loading on), in both
//!   channels: the corpus doubles as a test of the PUC chunk translators.
//!   `puc_bytecode_loads_in_every_dialect` then loads each dialect's
//!   chunks into Vms of the other four.
//! - `LUNA_DIFF_PUC_REQUIRE_ALL=1` — a missing interpreter or `luac`
//!   fails the run instead of skipping the dialect (the diff-puc workflow).
//!
//! Local dev: `brew install lua@5.4` (+ `lua` for 5.5); 5.1.5 /
//! 5.2.4 / 5.3.6 build in one `make macosx` each from the lua.org
//! tarballs — point the env vars at `src/lua`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use luna_core::__private::VmHostInternals;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Standalone, StandaloneOptions, Vm};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/diff_puc")
}

/// Each `tests/diff_puc/5.x/` runs on the matching `LuaVersion` and the
/// `PUC_LUA_5X` binary (5.5 falls back to `PUC_LUA`). A dialect whose
/// binary is absent is skipped with a notice (in CI, an install failure).
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
    if dialect == "5.5" && !require_all() {
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
        // `LUNA_DIFF_PUC_ONLY=<text>`: only the fixtures whose path has it
        let only = std::env::var("LUNA_DIFF_PUC_ONLY").ok();
        if p.extension().and_then(|s| s.to_str()) == Some("lua")
            && only.is_none_or(|o| p.to_string_lossy().contains(&o))
        {
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
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            assert!(
                !require_all(),
                "[diff_puc] cannot run PUC binary `{bin}`: {e}"
            );
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

/// Run `source` as `bin -` runs it, then close it as lua.c does; output caught.
fn run_on_luna(version: LuaVersion, bin: &str, source: &str) -> String {
    let (r, out) = super::stdout_capture::caught(|| {
        let mut s = lua_c_standalone(version, bin);
        run_as_script(&mut s, source).map_err(|e| e.to_string())
    }); // the Vm drops inside: what the close's finalizers print is caught
    r.expect("luna eval must not error on diff fixtures");
    String::from_utf8_lossy(&out).into_owned()
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
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            assert!(
                !require_all(),
                "[diff_puc] cannot run PUC binary `{bin}`: {e}"
            );
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

    let mut vm = lua_c_vm(version, bin);
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
    let luna = run_on_luna(version, bin, &source);
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
    // `LUNA_DIFF_PUC_KEEP_GOING=1`: every fixture runs, and the test fails
    // at the end with the list of those that diverged
    let keep_going = std::env::var_os("LUNA_DIFF_PUC_KEEP_GOING").is_some();
    let mut failed = Vec::new();
    for &(dialect, version, env_key) in DIALECTS {
        let fixtures = list_fixtures(dialect);
        if fixtures.is_empty() {
            continue;
        }
        let Some(bin) = puc_bin_for(dialect, env_key) else {
            assert!(
                !require_all(),
                "[diff_puc] dialect {dialect}: {env_key} must be set"
            );
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
            if !keep_going {
                diff_one(f, &bin, version);
            } else if std::panic::catch_unwind(|| diff_one(f, &bin, version)).is_err() {
                failed.push(f.display().to_string());
            }
        }
        total += fixtures.len();
    }
    assert!(
        total > 0,
        "no diff_puc fixtures ran (all dialects empty/skipped)"
    );
    assert!(
        failed.is_empty(),
        "{} diverged:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

/// `LUNA_DIFF_PUC_REQUIRE_ALL=1` (set by the diff-puc workflow) turns a
/// missing interpreter or `luac` into a failure: a run that silently
/// skipped a dialect would otherwise pass.
fn require_all() -> bool {
    std::env::var_os("LUNA_DIFF_PUC_REQUIRE_ALL").is_some()
}

pub(super) mod bytecode;

/// A Vm as lua.c sets one up for `bin -`: inside `pmain`, the libraries
/// open, `arg` made (5.3 on) and, from 5.4, the collector generational.
pub(super) fn lua_c_standalone(version: LuaVersion, bin: &str) -> Standalone {
    Vm::new_minimal(version)
        .into_standalone(&[bin, "-"], Some(1), StandaloneOptions::default())
        .expect("lua.c's state")
}

/// [`lua_c_standalone`]'s Vm.
pub(super) fn lua_c_vm(version: LuaVersion, bin: &str) -> Vm {
    lua_c_standalone(version, bin).into_vm()
}

/// lua.c's `handle_script` for `-`: `source` read from stdin, run by `docall`.
fn run_as_script(s: &mut Standalone, source: &str) -> Result<(), LuaError> {
    s.script_args()?;
    let vm = s.vm_mut();
    let f = vm.load_stdin_text(source.as_bytes().to_vec())?;
    let msgh = vm.native(keep_message);
    vm.call_value_with_handler_in_c(f, &[], msgh).map(|_| ())
}

/// The message handler, whose result the stdout channel never sees.
fn keep_message(_: &mut Vm, _: u32, _: u32) -> Result<u32, LuaError> {
    Ok(0)
}
