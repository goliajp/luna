//! v1.3 Phase AOT Stage 7 polish 3 — Windows COFF section emission +
//! deploy-side walker smoke.
//!
//! # What this test asserts
//!
//! The luna-aot pipeline targeting `x86_64-pc-windows-gnu` (MinGW)
//! produces a PE binary whose post-link section table contains:
//!
//! - `.lt_meta` — the trace-meta index (`AotTraceIndexEntry`),
//!   bracket-equivalent on Unix is `luna_trace_meta`. The Windows
//!   walker `luna-runtime-helpers::windows_section::find_section`
//!   looks for this exact 8-byte name.
//! - `.lt_skix` — the strkey resolver index. Bracket-equivalent on
//!   Unix is `luna_strkey_idx`.
//!
//! Both names come from `write_aot_cmain_object_for`'s Windows arm
//! (the placeholders that guarantee the sections exist even when the
//! binary linked zero AOT trace `.o`s). When MinGW gcc is on PATH,
//! the test runs the full link path; otherwise it skips with a clear
//! message.
//!
//! # E2E run-on-target?
//!
//! No. The cross-compile produces a Windows PE binary; running it
//! requires either a Windows host or an emulator (wine / qemu). This
//! test verifies the **emit** side only — that the PE structure
//! carries the expected sections in the expected shape. Running the
//! binary E2E is the manual verification recipe (see "Manual
//! verification" comment block at the bottom of this file).
//!
//! # Skip conditions
//!
//! - Host = Windows: this test is for the **cross-compile** path
//!   from a Unix host. Native Windows pipeline goes through a
//!   different driver setup (link.exe) that we don't yet drive.
//! - Missing `rustup` target `x86_64-pc-windows-gnu`: skipped with
//!   install hint.
//! - Missing `x86_64-w64-mingw32-gcc` on PATH: skipped (the staticlib
//!   build can still run, but without MinGW gcc the final link to
//!   produce the .exe is blocked).
//! - Harvest produces zero traces (small / non-loopy source): the
//!   produced binary still has the C placeholder sections; we assert
//!   their presence even when no real trace `.o` got linked in.

use std::path::Path;
use std::process::Command;

use luna_aot::embed::{TargetSpec, compile_and_link};
use luna_core::version::LuaVersion;

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
        return true;
    };
    if !output.status.success() {
        return true;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|l| l.trim() == triple)
}

/// Inspect the PE binary's section table and return the list of
/// section names found. Panics on parse failure (the binary should
/// always be a well-formed PE if the link step reported success).
fn read_pe_section_names(path: &Path) -> Vec<String> {
    use object::Object;
    use object::ObjectSection;
    use object::read::pe::PeFile64;

    let bytes = std::fs::read(path).expect("read produced PE binary");
    let pe = PeFile64::parse(&*bytes).expect("parse PE64 binary");
    pe.sections()
        .map(|s| {
            // Section names in the in-memory header are byte arrays
            // that may include trailing NUL pad; trim before storing.
            let raw = s.name_bytes().unwrap_or(b"");
            let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).into_owned()
        })
        .collect()
}

#[test]
fn cross_compile_windows_emits_lt_meta_and_lt_skix_sections() {
    if cfg!(target_os = "windows") {
        eprintln!(
            "stage7_windows_aot: skip — this test exercises the cross-compile path \
             from a Unix host. Native Windows pipeline uses link.exe (not yet wired)."
        );
        return;
    }
    if !have_on_path("cargo") {
        eprintln!("stage7_windows_aot: skip — cargo missing");
        return;
    }
    let triple = "x86_64-pc-windows-gnu";
    if !rustup_has_target(triple) {
        eprintln!(
            "stage7_windows_aot: skip — target {triple} not installed \
             (run `rustup target add {triple}` to enable)"
        );
        return;
    }
    if !have_on_path("x86_64-w64-mingw32-gcc") {
        eprintln!(
            "stage7_windows_aot: skip — x86_64-w64-mingw32-gcc not on PATH \
             (install MinGW cross-toolchain: `brew install mingw-w64` on macOS / \
             `apt install gcc-mingw-w64-x86-64` on Debian/Ubuntu). The staticlib \
             cross-build is verified separately by stage5_cross_compile."
        );
        return;
    }

    // Parse the spec for the assertion-side TargetOs check.
    let target = TargetSpec::from_triple(triple).expect("parse triple");
    assert_eq!(target.os, luna_aot::embed::TargetOs::Windows);

    // Lua source: a tight counted loop that the warmup recorder
    // should close at least one trace on. Note: trace mcode emission
    // is **host-only** (cranelift_native::builder()), so on a Unix
    // host targeting Windows we won't have a `.luna_traces.o` to
    // link. The C placeholder sections in the cmain shim are what
    // we're asserting — they exist unconditionally on Windows
    // targets.
    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("loop.lua");
    std::fs::write(
        &src_path,
        b"local s = 0\nfor i = 1, 1000 do s = s + i end\nprint(s)\n",
    )
    .expect("write source");

    let out_path = td.path().join("loop_aot_win.exe");
    let link_result = compile_and_link(&src_path, &out_path, Some(triple), LuaVersion::Lua55);
    let link_err = match link_result {
        Ok(()) => None,
        Err(e) => Some(format!("{e}")),
    };
    if let Some(msg) = link_err {
        // Mirror stage5_cross_compile's skip-marker pattern: a missing
        // rust-std / linker is a skip not a hard fail.
        let skip_markers = [
            "rustup target add",
            "can't find crate for `std`",
            "No such file or directory",
            "linker `cc` not found",
            "x86_64-w64-mingw32-gcc",
            "is incompatible with",
            "unsupported file format",
            "file not found",
            "fatal error",
        ];
        if skip_markers.iter().any(|m| msg.contains(m)) {
            eprintln!("stage7_windows_aot: skip — cross-toolchain incomplete: {msg}");
            return;
        }
        panic!(
            "stage7_windows_aot: unexpected link failure (none of the known skip \
             markers matched):\n{msg}"
        );
    }

    // PE magic sanity — first two bytes "MZ".
    let head = std::fs::read(&out_path).expect("read PE for magic check");
    assert!(head.len() > 1024, "produced PE suspiciously small");
    assert_eq!(&head[..2], b"MZ", "expected PE/DOS magic 'MZ'");

    // Inspect the section table.
    let names = read_pe_section_names(&out_path);
    let names_dbg = names.join(", ");
    assert!(
        names.iter().any(|n| n == ".lt_meta"),
        "expected section `.lt_meta` in linked PE; found sections: [{names_dbg}]"
    );
    assert!(
        names.iter().any(|n| n == ".lt_skix"),
        "expected section `.lt_skix` in linked PE; found sections: [{names_dbg}]"
    );

    eprintln!(
        "stage7_windows_aot: PE section table verified — found {} sections, \
         including `.lt_meta` and `.lt_skix`",
        names.len()
    );
}

// ────────────────────────────────────────────────────────────────────
// Manual verification recipe (when CI can't run wine/qemu):
//
// 1. Build on a Unix host with MinGW installed:
//      $ cargo run -p luna-aot --release -- compile loop.lua -o loop.exe \
//          --target x86_64-pc-windows-gnu
//    Expected: `loop.exe` produced, no link errors.
//
// 2. Verify section names via `llvm-readobj` (or `objdump -h`):
//      $ llvm-readobj --sections loop.exe | grep -E 'Name:.*lt_(meta|skix)'
//    Expected: two matches, `.lt_meta` and `.lt_skix`.
//
// 3. Run on Windows (or wine ≥ 8.0):
//      $ wine ./loop.exe
//    Expected: "500500" on stdout, exit 0.
//
// 4. With probe enabled to verify the section walker found entries:
//      $ wine sh -c "LUNA_AOT_PROBE=1 ./loop.exe"
//    Expected stderr lines:
//      - "aot_strkey_resolved = 0" (no AOT traces linked in for Unix-
//        host-cross targets, so the placeholder section yields 0)
//      - "aot_trace_install_count = 0"
//    A non-zero count would only appear when luna-aot itself runs on
//    a Windows host (so cranelift_native::builder() targets COFF and
//    the harvester actually emits a .o targeting the deploy binary's
//    PE format). Cross-targeted trace emission is out of scope.
// ────────────────────────────────────────────────────────────────────
