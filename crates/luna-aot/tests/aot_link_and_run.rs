//! Stage 4 smoke — full AOT pipeline drives a Lua source through
//! parse + compile + dump + bytecode-`.o` + C-main-`.o` +
//! `libluna_runtime_helpers.a` link → runs the produced native
//! binary in a clean subprocess and asserts stdout matches what the
//! script would print under the regular `Vm`.
//!
//! This is the end-to-end "does the AOT binary actually run?"
//! correctness signal for v1.3 Phase AOT Stage 4. Previous stages
//! only proved the section was reachable; this test proves
//! `print(...)` from inside the embedded chunk lands on the
//! subprocess's stdout via the staticlib-linked Vm.
//!
//! # Skip conditions
//!
//! The test is **conditional on a working host toolchain**. It skips
//! (via `eprintln!("skipped: …")` + early return — `cargo test`
//! reports it as `ok` either way, since cargo doesn't distinguish
//! skipped-but-asserted-skipped from passed) when:
//!
//! - `cc` is not on `PATH`
//! - `cargo` is not on `PATH` (we shell out to build the staticlib;
//!   in-tree test runs always satisfy this since `cargo test`
//!   guarantees `cargo` is reachable)
//! - Running on Windows (Stage 4 ships Unix-only — the embed.rs
//!   linker shim refuses Windows explicitly)
//!
//! The skip path keeps the test green on CI runners that strip
//! their build tools post-Rust-install.

use std::fs;
use std::path::Path;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;

/// Probe whether a binary is on PATH. Used for the conditional-skip
/// logic — see the module docs for skip rationale.
fn have_on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success() || o.status.code().is_some())
        .unwrap_or(false)
}

/// Run the produced AOT binary and return (stdout, stderr, exit_code).
fn run_binary(path: &Path) -> (String, String, Option<i32>) {
    let output = Command::new(path)
        .output()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", path.display()));
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

#[test]
fn aot_binary_runs_hello_print() {
    if cfg!(target_os = "windows") {
        eprintln!("skipped: Stage 4 Windows support is a follow-up");
        return;
    }
    if !have_on_path("cc") {
        eprintln!("skipped: `cc` not on PATH");
        return;
    }
    if !have_on_path("cargo") {
        eprintln!("skipped: `cargo` not on PATH");
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("hello.lua");
    fs::write(&src_path, b"print('hello from aot')\n").expect("write source");

    let out_path = td.path().join("hello_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55).unwrap_or_else(|e| {
        panic!("compile_and_link failed: {e}");
    });

    let meta = fs::metadata(&out_path).expect("output binary exists");
    assert!(meta.is_file(), "output is a regular file");
    assert!(meta.len() > 0, "output is non-empty");
    // sanity: the binary should be substantial (staticlib + bytecode +
    // C main + system libs); on macOS aarch64 this lands around 5-10 MB.
    // We assert > 100 KB so a degenerate stub-only binary (which would
    // be ~50 KB) fails loudly.
    assert!(
        meta.len() > 100 * 1024,
        "output binary is suspiciously small ({} bytes) — staticlib likely not linked",
        meta.len(),
    );

    let (stdout, stderr, code) = run_binary(&out_path);

    assert_eq!(
        code,
        Some(0),
        "binary exited non-zero (stdout: {stdout:?}, stderr: {stderr:?})"
    );
    assert_eq!(
        stdout, "hello from aot\n",
        "binary stdout mismatch (stderr: {stderr:?})"
    );
}

#[test]
fn aot_binary_runs_arithmetic_and_multi_print() {
    if cfg!(target_os = "windows") {
        eprintln!("skipped: Stage 4 Windows support is a follow-up");
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc/cargo not on PATH");
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("math.lua");
    fs::write(
        &src_path,
        b"local x = 2 + 3\nprint(x)\nprint('done', x * 2)\n",
    )
    .expect("write source");

    let out_path = td.path().join("math_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55).unwrap_or_else(|e| {
        panic!("compile_and_link failed: {e}");
    });

    let (stdout, stderr, code) = run_binary(&out_path);
    assert_eq!(code, Some(0), "stderr: {stderr:?}");
    // PUC `print` uses '\t' as the value separator and '\n' at end.
    assert_eq!(stdout, "5\ndone\t10\n", "stderr: {stderr:?}");
}

#[test]
fn aot_binary_propagates_runtime_error() {
    if cfg!(target_os = "windows") {
        eprintln!("skipped: Stage 4 Windows support is a follow-up");
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc/cargo not on PATH");
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("boom.lua");
    fs::write(&src_path, b"error('boom from script')\n").expect("write source");

    let out_path = td.path().join("boom_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55).unwrap_or_else(|e| {
        panic!("compile_and_link failed: {e}");
    });

    let (stdout, stderr, code) = run_binary(&out_path);
    assert_eq!(code, Some(1), "expected exit 1 on uncaught error");
    assert!(
        stderr.contains("boom from script"),
        "stderr should carry the lua error message, got: {stderr:?}"
    );
    // Nothing went to stdout — the script errored before printing.
    assert_eq!(stdout, "");
}
