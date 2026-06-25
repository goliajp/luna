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
