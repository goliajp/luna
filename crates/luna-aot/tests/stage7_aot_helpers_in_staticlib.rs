//! v1.3 Stage 7 follow-on — link-level smoke test for the
//! `luna_jit_*` helper symbol expose.
//!
//! Builds `libluna_runtime_helpers.a` via the same pipeline the AOT
//! `compile_and_link` flow uses (`cargo build -p luna-runtime-helpers
//! --profile=release-aot-helpers`), then shells out to `nm` and
//! asserts all 27 `luna_jit_*` Cranelift trace-mcode helpers are
//! present as defined-text (`T`) symbols.
//!
//! # Why this exists
//!
//! Per `.dev/rfcs/v1.3-rfc-trace-aot-relocation.md`, the deploy-side
//! staticlib that AOT binaries link against must carry every helper
//! the embedded `.o`'s Cranelift IR can call into. Workspace
//! `[profile.release]` has `lto = true`, which (correctly, from the
//! cross-crate optimizer's perspective) strips the helper-defining
//! cgus from the rlib → staticlib bundle because nothing in the
//! staticlib's Rust-side surface calls the helpers at run time.
//! The dedicated `release-aot-helpers` profile turns LTO off only for
//! this staticlib build to preserve the symbols.
//!
//! Without this test, a future PR that "cleans up" the profile back
//! to `--release` would silently regress: `cargo build` would
//! continue to succeed, `cargo test` would pass, and only the
//! *consumer-side* AOT binary link would fail at the user's machine
//! with `undefined reference to luna_jit_table_get_field`.
//!
//! # Skip conditions
//!
//! - `nm` not on PATH (Windows runners without llvm-nm in their
//!   toolchain — Stage 7 is Unix-floor anyway)
//! - `cargo` not on PATH (always present under `cargo test`)
//! - Building the helpers staticlib failed for unrelated reasons
//!   (rust-std missing for the host triple, etc.) — those surface
//!   as test failure rather than skip so they don't get silently
//!   masked.

use std::path::PathBuf;
use std::process::Command;

/// Probe whether a binary is on PATH.
fn have_on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Walk from `CARGO_MANIFEST_DIR` (= `crates/luna-aot/`) up to the
/// workspace root. Mirrors `luna-aot::embed::build_runtime_helpers_staticlib`'s
/// own ascent so the staticlib path resolution lines up.
fn workspace_root() -> PathBuf {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set under `cargo test`");
    PathBuf::from(&manifest_dir)
        .parent()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .expect("workspace root two levels up from crate manifest dir")
}

#[test]
fn all_27_luna_jit_helpers_are_defined_in_staticlib() {
    if !have_on_path("nm") {
        eprintln!("stage7_aot_helpers_in_staticlib: `nm` not on PATH, skipping");
        return;
    }
    if !have_on_path("cargo") {
        eprintln!("stage7_aot_helpers_in_staticlib: `cargo` not on PATH, skipping");
        return;
    }

    let root = workspace_root();

    // Build the staticlib via the same path `compile_and_link` uses.
    // We don't reach into `luna-aot::embed::build_runtime_helpers_staticlib`
    // because it's a private fn; shelling out keeps the test cheap and
    // matches the production pipeline byte-for-byte.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let build = Command::new(&cargo)
        .current_dir(&root)
        .arg("build")
        .arg("-p")
        .arg("luna-runtime-helpers")
        .arg("--profile=release-aot-helpers")
        // Avoid RUSTFLAGS pollution from any parent test harness
        // (matches `embed.rs::build_runtime_helpers_staticlib`).
        .env_remove("RUSTFLAGS")
        .output()
        .expect("spawn cargo build");
    assert!(
        build.status.success(),
        "cargo build -p luna-runtime-helpers --profile=release-aot-helpers failed:\n\
         stdout:\n{}\n\
         stderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let staticlib = root
        .join("target")
        .join("release-aot-helpers")
        .join("libluna_runtime_helpers.a");
    assert!(
        staticlib.exists(),
        "expected staticlib at {} after cargo build (cargo path layout drifted?)",
        staticlib.display(),
    );

    // `nm` reports `T <sym>` for defined-text on Linux ELF, and
    // `<addr> T _<sym>` for Mach-O (the `_` prefix is the platform
    // mangling for `extern "C"`). Both forms match the `_?luna_jit_`
    // grep below.
    let nm = Command::new("nm")
        .arg(&staticlib)
        .output()
        .expect("spawn nm");
    let stdout = String::from_utf8_lossy(&nm.stdout);
    // Tolerate non-zero exit when stdout still has content. Apple's
    // bundled llvm-nm (Xcode toolchain) is several LLVM versions behind
    // rustc's bitcode and emits per-member "Unknown attribute kind"
    // errors on every modern rlib member, then exits 1 — but its
    // stdout is still complete and correct for the symbols it could
    // parse (which includes all `T <sym>` entries we care about).
    //
    // Hard-fail only when both stdout is empty *and* exit was non-zero:
    // that's a real `nm` failure rather than version-skew chatter.
    assert!(
        nm.status.success() || !stdout.is_empty(),
        "nm produced no output on {} (exit={:?}, stderr={:?})",
        staticlib.display(),
        nm.status.code(),
        String::from_utf8_lossy(&nm.stderr),
    );

    // List of helpers — must match the
    // `luna-runtime-helpers/src/lib.rs::LUNA_AOT_HELPER_PIN` array and
    // the `pub unsafe extern "C" fn luna_jit_*` definitions in
    // `crates/luna-jit/src/jit_backend/mod.rs`. If a 28th helper lands
    // upstream, grow this list AND the pin array AND the
    // `force_link_jit_helpers_reports_27` test in luna-runtime-helpers.
    let expected: [&str; 27] = [
        "luna_jit_new_table",
        "luna_jit_new_table_sized",
        "luna_jit_materialize_sunk_table",
        "luna_jit_table_set_int",
        "luna_jit_table_set_raw",
        "luna_jit_table_set_field",
        "luna_jit_table_get_field",
        "luna_jit_op_get_tab_up",
        "luna_jit_table_set_nil",
        "luna_jit_table_set_float_float",
        "luna_jit_table_get_int",
        "luna_jit_table_get_float",
        "luna_jit_upval_get",
        "luna_jit_op_close",
        "luna_jit_stack_update_raw",
        "luna_jit_op_concat",
        "luna_jit_str_buf_acquire",
        "luna_jit_str_buf_release",
        "luna_jit_str_buf_extend",
        "luna_jit_str_buf_intern",
        "luna_jit_op_tforcall",
        "luna_jit_stack_load",
        "luna_jit_stack_tag",
        "luna_jit_spill_to_stack",
        "luna_jit_op_closure",
        "luna_jit_trace_materialize_frames",
        "luna_jit_table_len",
    ];

    let mut missing = Vec::new();
    for sym in expected.iter() {
        // Match either ` T <sym>` (Linux ELF) or ` T _<sym>` (Mach-O)
        // with a trailing word boundary (newline or whitespace) so
        // `luna_jit_op_concat` doesn't false-match
        // `luna_jit_op_concat_thing`.
        let elf = format!(" T {sym}\n");
        let macho = format!(" T _{sym}\n");
        if !stdout.contains(&elf) && !stdout.contains(&macho) {
            missing.push(*sym);
        }
    }

    assert!(
        missing.is_empty(),
        "staticlib `{}` is missing {} of 27 helper text symbols: {:?}\n\
         nm output (filtered for luna_jit_*):\n{}",
        staticlib.display(),
        missing.len(),
        missing,
        stdout
            .lines()
            .filter(|l| l.contains("luna_jit_"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
