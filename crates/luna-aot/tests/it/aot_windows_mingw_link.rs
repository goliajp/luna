//! Windows COFF section emission + deploy-side walker smoke.
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
//! On a Windows host the produced binary is run natively, with the AOT
//! probe on, and must print the sum and install at least one trace. On
//! a Unix host only the emit side is checked; aot-cross runs MinGW
//! binaries under Wine.
//!
//! # Skip conditions
//!
//! - Missing `rustup` target `x86_64-pc-windows-gnu`: skipped with
//!   install hint.
//! - No MinGW gcc on PATH (`x86_64-w64-mingw32-gcc`, or on a Windows
//!   host also plain `gcc`): skipped.
//! - On a Unix host, a link failing on a known cross-toolchain marker.
//!   With both tools present on a Windows host, any failure fails.

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
fn windows_gnu_binary_has_lt_meta_and_lt_skix_sections() {
    if !have_on_path("cargo") {
        eprintln!("aot_windows_mingw_link: skip — cargo missing");
        return;
    }
    let triple = "x86_64-pc-windows-gnu";
    if !rustup_has_target(triple) {
        eprintln!(
            "aot_windows_mingw_link: skip — target {triple} not installed \
             (run `rustup target add {triple}` to enable)"
        );
        return;
    }
    let has_gcc = have_on_path("x86_64-w64-mingw32-gcc") || (cfg!(windows) && have_on_path("gcc"));
    if !has_gcc {
        eprintln!(
            "aot_windows_mingw_link: skip — x86_64-w64-mingw32-gcc not on PATH \
             (install MinGW cross-toolchain: `brew install mingw-w64` on macOS / \
             `apt install gcc-mingw-w64-x86-64` on Debian/Ubuntu). The staticlib \
             cross-build is verified separately by aot_cross_compile."
        );
        return;
    }

    // Parse the spec for the assertion-side TargetOs check.
    let target = TargetSpec::from_triple(triple).expect("parse triple");
    assert_eq!(target.os, luna_aot::embed::TargetOs::Windows);

    // a hot counted loop the warmup recorder closes a trace on; the
    // sections checked below come from the C placeholders, so they are
    // there whether or not a trace `.o` was linked
    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("loop.lua");
    std::fs::write(
        &src_path,
        b"local s = 0\nfor i = 1, 1000000 do s = s + i end\nprint(s)\n",
    )
    .expect("write source");

    let out_path = td.path().join("loop_aot_win.exe");
    let link_result = compile_and_link(&src_path, &out_path, Some(triple), LuaVersion::Lua55);
    let link_err = match link_result {
        Ok(()) => None,
        Err(e) => Some(format!("{e}")),
    };
    if let Some(msg) = link_err {
        assert!(
            !cfg!(windows),
            "aot_windows_mingw_link: MinGW build on a Windows host failed:\n{msg}"
        );
        // Mirror aot_cross_compile's skip-marker pattern: a missing
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
            eprintln!("aot_windows_mingw_link: skip — cross-toolchain incomplete: {msg}");
            return;
        }
        panic!(
            "aot_windows_mingw_link: unexpected link failure (none of the known skip \
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

    if cfg!(windows) {
        let output = Command::new(&out_path)
            .env("LUNA_AOT_PROBE", "1")
            .output()
            .expect("run the MinGW binary");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "500000500000\n",
            "stderr: {stderr}"
        );
        assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
        let installed: usize = stderr
            .lines()
            .find_map(|l| l.split("aot_trace_install_count = ").nth(1))
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or_else(|| panic!("no install-count probe line; stderr:\n{stderr}"));
        assert!(installed >= 1, "no AOT trace installed; stderr:\n{stderr}");
    }

    eprintln!(
        "aot_windows_mingw_link: PE section table verified — found {} sections, \
         including `.lt_meta` and `.lt_skix`",
        names.len()
    );
}
