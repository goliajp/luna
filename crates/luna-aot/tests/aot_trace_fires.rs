//! v1.3 Phase AOT Stage 7 sub-piece 4 — end-to-end smoke for the
//! "AOT-compiled binary actually fires AOT trace mcode on a hot loop"
//! claim.
//!
//! # What this test asserts
//!
//! Source under test:
//!
//! ```lua
//! local s = 0
//! for i = 1, 1000000 do s = s + 1 end
//! print(s)
//! ```
//!
//! The 1,000,000-iteration counted loop is far above the trace JIT's
//! `TRACE_HOT_THRESHOLD = 64` back-edge count, so the warmup recorder
//! in [`luna_aot::embed::compile_and_link`] **must** close at least
//! one trace for the loop body and emit it into the
//! `luna_trace_meta` / `luna_trace_blob` sections.
//!
//! The deploy binary then:
//! 1. Walks `luna_trace_meta` at startup, matches each entry's
//!    `proto_hash` against its freshly-loaded chunk, and installs the
//!    AOT trace via `Vm::install_aot_trace`.
//! 2. Enters the dispatch loop. The first back-edge visit at the
//!    trace's `head_pc` finds the installed `CompiledTrace` and
//!    dispatches into the AOT mcode.
//! 3. The `LUNA_AOT_PROBE=1` hook in `Vm::run` (single-line
//!    `aot_trace_fired pc=N` on the first dispatch) confirms mcode
//!    actually executed.
//!
//! # Skip conditions
//!
//! - Windows: AOT trace install path is not implemented (COFF has no
//!   bracket-symbol convention).
//! - Missing `cc` or `cargo` on PATH: the AOT pipeline can't link.
//! - Cross-compile-only environments: this test only exercises the
//!   host triple.

use std::fs;
use std::path::Path;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;

fn have_on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success() || o.status.code().is_some())
        .unwrap_or(false)
}

fn run_with_env(path: &Path, env_key: &str, env_val: &str) -> (String, String, Option<i32>) {
    let output = Command::new(path)
        .env(env_key, env_val)
        .output()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", path.display()));
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

#[test]
fn aot_binary_fires_trace_mcode_on_hot_counted_loop() {
    if cfg!(target_os = "windows") {
        eprintln!(
            "skipped: AOT trace install requires bracket-symbol section convention \
             unavailable on Windows COFF"
        );
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("hot_loop.lua");
    // 1M-iteration counted loop; trace recorder hits TRACE_HOT_THRESHOLD
    // (64) after the first 64 back-edges and closes the body trace
    // well before iteration 1000.
    fs::write(
        &src_path,
        b"local s = 0\n\
          for i = 1, 1000000 do s = s + 1 end\n\
          print(s)\n",
    )
    .expect("write source");

    let out_path = td.path().join("hot_loop_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55).unwrap_or_else(|e| {
        panic!("compile_and_link failed: {e}");
    });

    let (stdout, stderr, code) = run_with_env(&out_path, "LUNA_AOT_PROBE", "1");

    assert_eq!(
        code,
        Some(0),
        "binary exited non-zero (stdout: {stdout:?}, stderr: {stderr:?})"
    );
    assert_eq!(
        stdout, "1000000\n",
        "counted loop produced wrong stdout (stderr: {stderr:?})"
    );

    // Two probe assertions: install ran (count > 0 means at least one
    // trace was warmup-recorded AND AOT-installed) AND the dispatcher
    // actually fired the trace mcode at least once.
    //
    // `aot_trace_install_count = N` (N >= 1) confirms the meta walker
    // matched at least one trace.
    let install_line = stderr
        .lines()
        .find(|l| l.contains("aot_trace_install_count = "))
        .unwrap_or_else(|| panic!("expected install-count probe line; stderr:\n{stderr}"));
    let install_count: usize = install_line
        .rsplit(" = ")
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| panic!("could not parse install count from {install_line:?}"));
    assert!(
        install_count >= 1,
        "expected at least 1 AOT trace installed (would mean warmup recorded zero traces \
         — TRACE_HOT_THRESHOLD tuning issue?); install_count={install_count}, stderr:\n{stderr}"
    );

    // `aot_trace_fired pc=N` confirms mcode dispatched. The probe
    // emits on the FIRST dispatch only (counters.dispatched == 0
    // guard), so exactly one occurrence is expected.
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "expected aot_trace_fired probe (AOT mcode dispatched at least once); \
         got stderr:\n{stderr}"
    );
}

#[test]
fn aot_binary_zero_traces_on_trivial_source_still_runs() {
    // Mirror image: tiny straight-line source the warmup recorder
    // cannot turn into a trace. The pipeline should produce a
    // runnable binary (no link failure on missing trace .o; the
    // cmain shim's `luna_trace_meta` placeholder keeps the bracket
    // symbols defined), and run it should print + exit cleanly
    // without any `aot_trace_fired` probe.
    if cfg!(target_os = "windows") {
        eprintln!("skipped: see other test");
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("trivial.lua");
    fs::write(&src_path, b"print('no traces here')\n").expect("write source");
    let out_path = td.path().join("trivial_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55).unwrap_or_else(|e| {
        panic!("compile_and_link failed: {e}");
    });
    let (stdout, stderr, code) = run_with_env(&out_path, "LUNA_AOT_PROBE", "1");
    assert_eq!(code, Some(0), "stderr: {stderr:?}");
    assert_eq!(stdout, "no traces here\n", "stderr: {stderr:?}");
    // Install-count probe still fires, value should be 0.
    let install_line = stderr
        .lines()
        .find(|l| l.contains("aot_trace_install_count = "))
        .unwrap_or_else(|| panic!("expected install-count probe line; stderr:\n{stderr}"));
    assert!(
        install_line.ends_with("= 0"),
        "expected 0 traces installed for trivial source; got: {install_line:?}"
    );
    assert!(
        !stderr.contains("aot_trace_fired"),
        "no AOT mcode should dispatch on trivial source; stderr:\n{stderr}"
    );
}
