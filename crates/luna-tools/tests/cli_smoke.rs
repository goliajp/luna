//! v2.0 Track TL Phase 1 — CLI smoke harness.
//!
//! For every binary defined in `Cargo.toml`'s `[[bin]]` entries,
//! assert `<binary> --help` exits 0 and contains the binary name.
//! Stubs that exit non-zero on real input (`unimplemented!`) still
//! return 0 on `--help` (clap intercepts before `main` runs); this
//! pins that contract.

use std::path::PathBuf;
use std::process::Command;

fn binary_path(name: &str) -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo for `tests/*.rs`
    // targets. The mapping uses the `[[bin]]` `name` field
    // verbatim (hyphens preserved).
    let var = format!("CARGO_BIN_EXE_{name}");
    PathBuf::from(std::env::var_os(&var).unwrap_or_else(|| {
        panic!("{var} not set — re-check the `[[bin]] name = \"{name}\"` entry in Cargo.toml")
    }))
}

fn assert_help_ok(name: &str) {
    let path = binary_path(name);
    let out = Command::new(&path)
        .arg("--help")
        .output()
        .unwrap_or_else(|e| panic!("running {} --help: {e}", path.display()));
    assert!(
        out.status.success(),
        "{name} --help exited non-zero: status={:?} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // clap renders the binary name at the top of `--help` output.
    assert!(
        stdout.contains(name),
        "{name} --help did not mention its own name; got:\n{stdout}"
    );
}

#[test]
fn help_luna_bin_inspect() {
    assert_help_ok("luna-bin-inspect");
}

#[test]
fn help_luna_heap_dump() {
    assert_help_ok("luna-heap-dump");
}

#[test]
fn help_luna_profile() {
    assert_help_ok("luna-profile");
}

#[test]
fn help_luna_trace_inspect() {
    assert_help_ok("luna-trace-inspect");
}

#[test]
fn help_luna_repl_polish() {
    assert_help_ok("luna-repl-polish");
}

#[test]
fn luna_heap_dump_runs_toy_script() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("toy.lua");
    std::fs::write(
        &script_path,
        "local t = {}; for i = 1, 5 do t[i] = i * 2 end; return t\n",
    )
    .expect("write script");

    let path = binary_path("luna-heap-dump");
    let out = Command::new(&path)
        .arg(&script_path)
        .arg("--out")
        .arg("json")
        .output()
        .unwrap_or_else(|e| panic!("running heap-dump: {e}"));
    assert!(
        out.status.success(),
        "heap-dump exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // JSON shape sanity.
    assert!(stdout.contains("\"schema_version\":"));
    assert!(stdout.contains("\"buckets\":"));
    // A fresh Vm always preloads enough stdlib for at least one
    // table + several strings.
    assert!(stdout.contains("\"table\""));
    assert!(stdout.contains("\"str\""));
}

#[test]
fn luna_bin_inspect_runs_on_self() {
    // Inspecting `luna-bin-inspect` itself (a non-AOT binary)
    // must succeed and report "0" AOT trace entries — not error
    // out. This is the graceful-degradation contract for users
    // who pass the wrong file in.
    let target = binary_path("luna-bin-inspect");
    let path = target.clone();
    let out = Command::new(&path)
        .arg(&target)
        .output()
        .unwrap_or_else(|e| panic!("running bin-inspect: {e}"));
    assert!(
        out.status.success(),
        "bin-inspect exited non-zero on self: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("format:"));
    assert!(stdout.contains("AOT trace index entries:"));
}

#[test]
fn luna_trace_inspect_runs_simple_script() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("loop.lua");
    // A small loop that gives the trace recorder a chance to engage
    // but doesn't bog the test down. We don't assert it actually
    // traced — only that the counter dump is well-formed.
    std::fs::write(
        &script_path,
        "local s = 0; for i = 1, 50000 do s = s + i end; return s\n",
    )
    .expect("write script");

    let path = binary_path("luna-trace-inspect");
    let out = Command::new(&path)
        .arg(&script_path)
        .arg("--format")
        .arg("json")
        .output()
        .unwrap_or_else(|e| panic!("running trace-inspect: {e}"));
    assert!(
        out.status.success(),
        "trace-inspect exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"schema\": \"luna-trace-inspect.v1\""));
    assert!(stdout.contains("\"counters\":"));
    assert!(stdout.contains("\"trace_compiled\":"));
    assert!(stdout.contains("\"trace_enabled\": true"));
}

#[test]
fn luna_trace_inspect_rejects_show_ir_for_track_r() {
    // --show ir is reserved for Track R IR shape stabilising
    // (audit R1). Confirm the binary still exits non-zero with
    // a tracking-doc pointer instead of pretending to render.
    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("noop.lua");
    std::fs::write(&script_path, "return 1\n").expect("write script");
    let path = binary_path("luna-trace-inspect");
    let out = Command::new(&path)
        .arg(&script_path)
        .arg("--show")
        .arg("ir")
        .output()
        .unwrap_or_else(|e| panic!("running trace-inspect: {e}"));
    assert!(!out.status.success(), "expected --show ir to fail today");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Track R") || stderr.contains("audit R1"),
        "expected R1 tracking pointer; got stderr:\n{stderr}"
    );
}

#[test]
fn luna_profile_collects_samples() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("hot.lua");
    // A deeply-nested hot loop so the Count hook (at --every 100)
    // gets several ticks across at least two distinct frames.
    std::fs::write(
        &script_path,
        "local function inner(n) local s = 0 \
         for i = 1, n do s = s + i end return s end\n\
         local function outer() local t = 0 \
         for j = 1, 100 do t = t + inner(1000) end return t end\n\
         return outer()\n",
    )
    .expect("write script");

    let path = binary_path("luna-profile");
    let out = Command::new(&path)
        .arg(&script_path)
        .arg("--every")
        .arg("100")
        .output()
        .unwrap_or_else(|e| panic!("running profile: {e}"));
    assert!(
        out.status.success(),
        "profile exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("luna-profile"));
    assert!(stdout.contains("samples:"));
    // The hot loop is comfortably bigger than 100 instructions,
    // so we expect ≥1 sample. Don't pin a higher floor — the
    // exact count depends on dispatch policy.
    let parses_nonzero = stdout
        .lines()
        .find(|l| l.contains("samples:"))
        .map(|l| {
            l.split_whitespace()
                .find_map(|tok| tok.parse::<u64>().ok())
                .unwrap_or(0)
        })
        .unwrap_or(0)
        > 0;
    assert!(parses_nonzero, "expected ≥1 sample; got stdout:\n{stdout}");
}

#[test]
fn luna_profile_folded_emits_inferno_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script_path = dir.path().join("hot2.lua");
    // Wrap the hot loop in a function so the frame stack has at
    // least one Lua activation when the Count hook ticks (the
    // top-level chunk activation collapses on Return; mid-chunk
    // dispatch may register zero frames).
    std::fs::write(
        &script_path,
        "local function hot() local s = 0 \
         for i = 1, 100000 do s = s + i end return s end\n\
         return hot()\n",
    )
    .expect("write script");

    let path = binary_path("luna-profile");
    let out = Command::new(&path)
        .arg(&script_path)
        .arg("--every")
        .arg("100")
        .arg("--format")
        .arg("folded")
        .output()
        .unwrap_or_else(|e| panic!("running profile: {e}"));
    assert!(
        out.status.success(),
        "profile --format folded exited non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Each folded line ends with a space + integer count, and
    // references the temp script's path via a `source:line` token.
    let saw_line = stdout.lines().any(|line| {
        line.contains("hot2.lua")
            && line
                .rsplit_once(' ')
                .and_then(|(_, n)| n.parse::<u64>().ok())
                .is_some()
    });
    assert!(
        saw_line,
        "expected ≥1 folded line referencing hot2.lua; got:\n{stdout}"
    );
}
