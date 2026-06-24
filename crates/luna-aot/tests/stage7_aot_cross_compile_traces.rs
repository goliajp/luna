//! v1.3 Phase AOT Stage 7 polish 4 — cross-compile AOT traces.
//!
//! Before this polish the offline trace recorder (`harvest_and_emit_
//! aot_traces`) built its `ObjectModule` against the **host** Cranelift
//! ISA. Cross-built binaries (`--target x86_64-unknown-linux-musl` from
//! an aarch64-apple-darwin host) therefore landed without any AOT mcode
//! in `luna_trace_meta` — they ran the embedded bytecode through
//! interp + runtime JIT only, dropping the AOT fast-path entirely.
//!
//! This test asserts the polish: cross-compile a hot-loop chunk for
//! `x86_64-unknown-linux-musl`, then walk the produced binary's
//! `luna_trace_meta` section with the `object` crate and check the
//! number of 48-byte entries is **non-zero**. The deploy walker reads
//! the same section at startup to install the AOT traces, so a
//! non-empty `luna_trace_meta` is the loadable AOT-fast-path signal.
//!
//! # What we do **not** verify
//!
//! - **Execution**: running the produced x86_64 ELF on an aarch64 mac
//!   needs qemu / docker / Rosetta; out of scope. Stage 6 has the
//!   docker-based Alpine smoke that does that for the bytecode-interp
//!   path; the AOT-trace dispatch fires via the same runtime hook so
//!   adding it to that test is the natural follow-up.
//! - **mcode bytes**: we don't disassemble the trace function body.
//!   Cross-codegen correctness at the mcode level is Cranelift's
//!   responsibility — `cranelift-codegen = { features = ["all-arch"] }`
//!   pulls in the per-arch ISA tables, and the same ISA powers
//!   `wasmtime`'s production cross-codegen path.
//!
//! # Skip conditions
//!
//! Mirror `stage5_cross_compile.rs`'s skip taxonomy:
//! - Missing `cc` / `cargo` on PATH (won't compile the staticlib).
//! - `rustup target add x86_64-unknown-linux-musl` not run.
//! - Cross-cc missing (`musl-gcc` / `x86_64-linux-musl-gcc`) → the
//!   link step fails with a recognised marker, and we skip rather
//!   than fail.

use std::path::Path;
use std::process::Command;

use luna_aot::embed::{TargetSpec, compile_and_link};
use luna_core::version::LuaVersion;

const HOT_LOOP_LUA: &[u8] = b"local s = 0\nfor i = 1, 1000000 do s = s + 1 end\nprint(s)\n";

fn have_on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success() || o.status.code().is_some())
        .unwrap_or(false)
}

fn rustup_has_target(triple: &str) -> bool {
    let Ok(output) = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    else {
        return true; // no rustup → don't pre-emptively skip
    };
    if !output.status.success() {
        return true;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().any(|l| l.trim() == triple)
}

/// Walk the produced binary with the `object` crate and count entries
/// in `luna_trace_meta`. Each entry is 48 bytes (matches
/// `AotTraceIndexEntry::SIZE` in luna-core).
///
/// `entry_count > 0` means at least one AOT trace landed in the
/// binary's deploy-time installable set (the `__DATA,luna_trace_meta`
/// section on Mach-O, `luna_trace_meta` on ELF). The placeholder
/// 48-byte zero entry from `write_aot_cmain_object_for` is counted
/// here too — the deploy walker skips it via `entry.fn_ptr.is_null()`
/// — so we assert `> 1` (= placeholder + at least one real trace).
fn count_luna_trace_meta_entries(binary: &Path) -> Result<usize, String> {
    use object::{Object, ObjectSection};
    let bytes = std::fs::read(binary).map_err(|e| format!("read binary: {e}"))?;
    let obj = object::File::parse(&*bytes).map_err(|e| format!("parse object: {e}"))?;
    for section in obj.sections() {
        let Ok(name) = section.name() else { continue };
        if name == "luna_trace_meta" || name.ends_with(",luna_trace_meta") {
            let data = section
                .data()
                .map_err(|e| format!("read section data: {e}"))?;
            if data.len() % 48 != 0 {
                return Err(format!(
                    "luna_trace_meta size {} not a multiple of 48",
                    data.len()
                ));
            }
            return Ok(data.len() / 48);
        }
    }
    Err("no luna_trace_meta section in produced binary".to_string())
}

#[test]
fn target_spec_resolves_cross_cranelift_isa() {
    // Pure unit-style: no toolchain. Verifies the new
    // `cranelift_isa_builder` returns a builder for every tier-1/2
    // arch we promise to cross-codegen for.
    for triple in [
        "x86_64-unknown-linux-musl",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-unknown-linux-musl",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ] {
        let spec = TargetSpec::from_triple(triple)
            .unwrap_or_else(|e| panic!("triple {triple} should parse: {e}"));
        let builder = spec.cranelift_isa_builder().unwrap_or_else(|e| {
            panic!(
                "cranelift_isa_builder failed for {triple}: {e} \
                 (cranelift-codegen features may have shrunk — \
                 ensure `all-arch` is on)"
            )
        });
        // `triple()` access proves the builder wired the triple
        // through. We don't `finish()` (would need a Flags) — that's
        // exercised by `cross_compile_emits_trace_mcode` below.
        let resolved = builder.triple();
        // For darwin Apple's triple normalises to drop the apple
        // vendor; for linux-musl the env shows up as part of the env
        // field. Compare loosely via arch prefix.
        let arch_match = triple.starts_with("x86_64")
            && resolved.architecture.to_string() == "x86_64"
            || triple.starts_with("aarch64")
                && format!("{}", resolved.architecture).starts_with("aarch64");
        assert!(
            arch_match,
            "ISA builder triple arch {:?} does not match expected for {triple}",
            resolved.architecture
        );
    }
}

/// Drive `triple` through `compile_and_link` and assert the produced
/// binary's `luna_trace_meta` carries at least one real (non-placeholder)
/// AOT trace entry. Returns `Ok(entry_count)` on success, `Err(msg)` for
/// the cross-toolchain skip taxonomy mirroring `stage5_cross_compile.rs`.
fn try_emit_traces_for(triple: &str) -> Result<usize, String> {
    let td = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let src_path = td.path().join("hot.lua");
    std::fs::write(&src_path, HOT_LOOP_LUA).map_err(|e| format!("write source: {e}"))?;
    let out_path = td
        .path()
        .join(format!("hot_aot_{}", triple.replace('-', "_")));

    match compile_and_link(&src_path, &out_path, Some(triple), LuaVersion::Lua55) {
        Ok(()) => {
            let entries = count_luna_trace_meta_entries(&out_path)?;
            // Keep tempdir alive while we read the binary; std::fs::read
            // completed inside count_*; the function returned the count.
            // Returning the count drops `td` after, which is fine.
            Ok(entries)
        }
        Err(e) => {
            let msg = format!("{e}");
            let skip_markers = [
                "rustup target add",
                "can't find crate for `std`",
                "No such file or directory",
                "ld: unknown options",
                "ld: warning: ignoring file",
                "unsupported file format",
                "unknown argument",
                "invalid linker",
                "linker `cc` not found",
                "error: linker",
                "is incompatible with",
                "in function `_start'",
                "'stddef.h' file not found",
                "file not found",
                "fatal error",
                "musl-gcc",
            ];
            if skip_markers.iter().any(|m| msg.contains(m)) {
                Err(format!("skip: {msg}"))
            } else {
                Err(format!("hard-fail: {msg}"))
            }
        }
    }
}

#[test]
fn cross_compile_emits_trace_mcode_for_x86_64_apple_darwin() {
    // This is the strongest end-to-end signal for polish 4 on a
    // macOS aarch64 host: Apple's clang handles `-target x86_64-apple-
    // darwin` natively (no extra toolchain install), so the link step
    // actually completes and the test asserts non-empty trace mcode
    // landed in the cross-built binary.
    if cfg!(target_os = "windows") {
        eprintln!("skipped: Windows host");
        return;
    }
    if cfg!(not(target_os = "macos")) {
        eprintln!(
            "skipped: cross-to-darwin requires the macOS SDK; only meaningful on \
             a macOS host"
        );
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }

    const TRIPLE: &str = "x86_64-apple-darwin";
    if !rustup_has_target(TRIPLE) {
        eprintln!(
            "skipped: target {TRIPLE} not installed (run `rustup target add {TRIPLE}` to enable)"
        );
        return;
    }

    match try_emit_traces_for(TRIPLE) {
        Ok(entries) => {
            assert!(
                entries > 1,
                "cross-compiled {TRIPLE} binary should have placeholder + ≥1 real \
                 AOT trace entry; got {entries}. polish 4 may have regressed: \
                 the cross-codegen path is short-circuiting to interp-only."
            );
            eprintln!(
                "stage7-polish-4: {TRIPLE} produced binary with {entries} \
                 luna_trace_meta entries (placeholder + {} real trace(s))",
                entries - 1,
            );
        }
        Err(msg) if msg.starts_with("skip:") => {
            eprintln!("skipped: cross-toolchain shortfall for {TRIPLE}: {msg}");
        }
        Err(msg) => {
            panic!(
                "cross-compile for {TRIPLE} failed unexpectedly: {msg}\n\
                 (no known skip marker — the AOT pipeline itself may be broken)"
            );
        }
    }
}

#[test]
fn cross_compile_emits_trace_mcode_for_linux_musl_x86_64() {
    if cfg!(target_os = "windows") {
        eprintln!(
            "skipped: AOT trace install requires bracket-symbol section convention \
             unavailable on Windows COFF; cross-compile-to-linux path also driver-different"
        );
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }

    const TRIPLE: &str = "x86_64-unknown-linux-musl";
    if !rustup_has_target(TRIPLE) {
        eprintln!(
            "skipped: target {TRIPLE} not installed (run `rustup target add {TRIPLE}` to enable)"
        );
        return;
    }

    match try_emit_traces_for(TRIPLE) {
        Ok(entries) => {
            assert!(
                entries > 1,
                "cross-compiled {TRIPLE} binary should have placeholder + ≥1 real \
                 AOT trace entry; got {entries}. polish 4 may have regressed."
            );
            eprintln!(
                "stage7-polish-4: {TRIPLE} produced binary with {entries} \
                 luna_trace_meta entries (placeholder + {} real trace(s))",
                entries - 1,
            );
        }
        Err(msg) if msg.starts_with("skip:") => {
            eprintln!("skipped: cross-toolchain shortfall for {TRIPLE}: {msg}");
        }
        Err(msg) => {
            panic!(
                "cross-compile for {TRIPLE} failed unexpectedly: {msg}\n\
                 (no known skip marker — the AOT pipeline itself may be broken)"
            );
        }
    }
}
