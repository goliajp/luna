//! v1.3 Phase AOT Stage 7 trace-coverage follow-up — end-to-end smoke
//! for the wire-format v2 cut that extends AOT trace installs to
//! traces carrying typed-register side-exits (per_exit_tags).
//!
//! # What this test asserts
//!
//! Source under test:
//!
//! ```lua
//! local t = {x = 0}
//! for i = 1, 1000000 do t.x = t.x + i end
//! print(t.x)
//! ```
//!
//! The body uses GetField + SetField against a table upvalue from
//! the enclosing scope (the local `t`). The trace recorder emits
//! type-check guards on `t.x`'s register, which the lowerer turns
//! into a per-cont_pc side-exit — landing the closed trace's
//! `per_exit_tags` non-empty. Under wire format v1 this trace was
//! filtered out at the AOT harvester ("`has_per_exit_tags`"); under
//! v2 the harvester emits the trace and the deploy walker
//! reconstructs `per_exit_tags` for the install, so the dispatcher
//! restores the right slot shapes on the side-exit path.
//!
//! The test asserts:
//! 1. The AOT binary exits clean with the right sum on stdout.
//! 2. At least one AOT trace was installed.
//! 3. AOT mcode actually dispatched (probe line present).
//!
//! # Skip conditions — mirror stage7_aot_trace_fires.rs.

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
fn aot_binary_fires_trace_mcode_on_getfield_setfield_loop() {
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
    let src_path = td.path().join("getfield_loop.lua");
    // Counted sum loop over a table-field upvalue. The expected sum
    // is `n * (n + 1) / 2` for n = 1_000_000 = 500_000_500_000.
    fs::write(
        &src_path,
        b"local t = {x = 0}\n\
          for i = 1, 1000000 do t.x = t.x + i end\n\
          print(t.x)\n",
    )
    .expect("write source");

    let out_path = td.path().join("getfield_loop_aot");
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
        stdout, "500000500000\n",
        "table-field sum loop produced wrong stdout (stderr: {stderr:?})"
    );

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
        "expected at least 1 AOT trace installed for GetField/SetField loop; \
         install_count={install_count}, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "expected aot_trace_fired probe (AOT mcode dispatched at least once); \
         got stderr:\n{stderr}"
    );
}
