//! v1.3 Phase AOT Stage 7 sub-piece 3 — deploy-side string-key
//! resolver smoke test.
//!
//! # What sub-piece 3 changes
//!
//! Sub-piece 2 (commit `1523f03`) made the trace lowerer emit
//! `__luna_aot_strkey_{slot,bytes,idx}_<hex>` data symbols when
//! `CompileOptions { aot: true }` is set. The slots are
//! zero-initialised; the trace mcode loads through them and would
//! dereference NULL on the first dispatch without a deploy-side
//! resolver.
//!
//! Sub-piece 3 lands two pieces:
//!
//! 1. A dedicated `luna_strkey_idx` section the lowerer fills with
//!    `[bytes_addr, slot_addr]` 16-byte entries (one per unique
//!    key, deduped within a single `lower_trace_into` call).
//! 2. A deploy-side `aot_strkey_resolver::resolve_all(&mut Vm)` fn
//!    in `luna-runtime-helpers` that walks the bracketed section
//!    range (`__start_luna_strkey_idx` / `__stop_luna_strkey_idx`
//!    on ELF, `section$start$__DATA$luna_strkey_idx` on Mach-O),
//!    interns each bytes block into the deploy `Vm`'s heap, and
//!    writes the resulting `Gc<LuaStr>::as_ptr()` into the matching
//!    slot.
//!
//! The C-main shim in `luna-aot::embed` also emits an empty
//! placeholder of the `luna_strkey_idx` section so the bracket
//! symbols resolve even when zero trace `.o`s linked in (the
//! sub-piece-4 path that emits trace mcode is the next session's
//! work; until then, every AOT binary has zero trace .o's and the
//! resolver returns 0).
//!
//! # What this test asserts
//!
//! 1. An AOT binary produced via `compile_and_link` runs cleanly
//!    with `LUNA_AOT_PROBE=1` set (no segfault on the resolver
//!    walk, no link failure on the bracket symbols).
//! 2. The probe line `luna-runtime-helpers: aot_strkey_resolved = N`
//!    appears in stderr, confirming the resolver entry point was
//!    reached. `N == 0` is acceptable today (no trace `.o`s linked
//!    yet) — `N > 0` would prove the section-walk + intern + slot-
//!    write loop body works against real entries, but that requires
//!    sub-piece 4 (trace .o emission).
//!
//! # What this test does NOT prove
//!
//! - End-to-end "AOT trace mcode fires through resolver-populated
//!   slot". Requires sub-piece 4 (the (Proto, pc) → mcode dispatch
//!   registry). The resolver code path runs in either case, but
//!   the non-empty walk only matters once sub-piece 4 emits trace
//!   `.o`s. See `crates/luna-runtime-helpers/src/lib.rs::
//!   aot_trace_registry` for the sub-piece 4 plan.
//! - Cross-platform bracket-symbol correctness. The Mach-O
//!   `section$start$...$<sect>` and ELF `__start_<sect>` forms are
//!   declared per-`cfg!`; this test exercises whichever the host
//!   is. CI matrix (already covered by `stage5_cross_compile` /
//!   `stage6_alpine_smoke`) will catch the cross variants.

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
fn aot_binary_resolver_runs_clean() {
    if cfg!(target_os = "windows") {
        eprintln!(
            "skipped: Stage 7 sub-piece 3 placeholder is Unix-only \
             (Windows COFF has no bracket convention; resolver no-ops)"
        );
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    // A script with table getfield/setfield so a future sub-piece-4
    // session that wires trace emission will exercise the resolver
    // against real entries. Today the script runs through interp,
    // so no traces emit; the test asserts the resolver entry path
    // is reachable, not that any slot got populated.
    let src_path = td.path().join("table_ops.lua");
    fs::write(
        &src_path,
        b"local t = { field = 42 }\n\
          t.other = t.field + 1\n\
          print(t.field, t.other)\n",
    )
    .expect("write source");

    let out_path = td.path().join("table_ops_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55).unwrap_or_else(|e| {
        panic!("compile_and_link failed: {e}");
    });

    let (stdout, stderr, code) = run_with_env(&out_path, "LUNA_AOT_PROBE", "1");

    assert_eq!(
        code,
        Some(0),
        "binary exited non-zero with resolver probe (stdout: {stdout:?}, \
         stderr: {stderr:?})"
    );
    assert_eq!(
        stdout, "42\t43\n",
        "binary stdout mismatch — table ops broke or resolver corrupted \
         the heap (stderr: {stderr:?})"
    );
    // The resolver probe line. `N` can be 0 (no trace .o's linked
    // today, sub-piece 4 pending) or positive (future-proof for the
    // session that wires trace .o emission).
    assert!(
        stderr.contains("aot_strkey_resolved = "),
        "expected resolver probe line in stderr, got: {stderr:?}"
    );
}

#[test]
fn aot_binary_no_probe_no_diagnostic() {
    // Mirror image: with `LUNA_AOT_PROBE` unset, the resolver runs
    // silently. Confirms the probe line is gated, not always-emitted
    // (a regression here would pollute every AOT binary's stderr).
    if cfg!(target_os = "windows") {
        eprintln!("skipped: see other test");
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("quiet.lua");
    fs::write(&src_path, b"print('quiet')\n").expect("write source");

    let out_path = td.path().join("quiet_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55).unwrap_or_else(|e| {
        panic!("compile_and_link failed: {e}");
    });

    let output = Command::new(&out_path).output().expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr:?}");
    assert_eq!(stdout, "quiet\n", "stderr: {stderr:?}");
    assert!(
        !stderr.contains("aot_strkey_resolved"),
        "resolver probe should be silent without LUNA_AOT_PROBE; got stderr: {stderr:?}"
    );
}
