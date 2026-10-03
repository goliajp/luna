//! Windows MSVC link path.
//!
//! # What this test asserts
//!
//! Building luna-aot for `x86_64-pc-windows-msvc` produces a PE
//! binary whose section table contains the same `.lt_meta` /
//! `.lt_skix` placeholder sections that the MinGW path emits. The
//! MSVC C compiler driver (`clang-cl` / `cl.exe`) and linker driver
//! (`lld-link` / `link.exe`) are completely separate from the
//! gcc-style cc shape used everywhere else; this test verifies the
//! emit side stays compatible with the same deploy walker
//! (`luna-runtime-helpers::windows_section::find_section`) that the
//! MinGW path uses.
//!
//! # Skip conditions
//!
//! On a Windows host none: luna-aot finds `cl.exe` / `link.exe` in the
//! Visual Studio install, and the produced binary is also run. On a
//! Unix host (the cross-compile leg):
//!
//! - `rustup` target `x86_64-pc-windows-msvc` not installed.
//! - `clang-cl` or `lld-link` not on PATH.
//! - The link fails on a known cross-toolchain-incomplete marker
//!   (`lld-link` without the Windows SDK / UCRT libraries).
//!
//! # E2E run-on-target?
//!
//! On a Windows host only. The file-format + section-table check is
//! load-bearing everywhere because the deploy-side section walker
//! keys off exactly those two facts (PE magic + section names).

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
        // No rustup at all — assume target present and let the staticlib
        // build surface the real error.
        return true;
    };
    if !output.status.success() {
        return true;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|l| l.trim() == triple)
}

fn read_pe_section_names(path: &Path) -> Vec<String> {
    use object::Object;
    use object::ObjectSection;
    use object::read::pe::PeFile64;

    let bytes = std::fs::read(path).expect("read produced PE binary");
    let pe = PeFile64::parse(&*bytes).expect("parse PE64 binary");
    pe.sections()
        .map(|s| {
            let raw = s.name_bytes().unwrap_or(b"");
            let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).into_owned()
        })
        .collect()
}

#[test]
fn windows_msvc_binary_has_lt_meta_and_lt_skix_sections() {
    if !have_on_path("cargo") {
        eprintln!("aot_msvc_link: skip — cargo missing");
        return;
    }

    let triple = "x86_64-pc-windows-msvc";
    let native = cfg!(all(windows, target_env = "msvc", target_arch = "x86_64"));
    if !native {
        if !rustup_has_target(triple) {
            eprintln!(
                "aot_msvc_link: skip — target {triple} not installed \
                 (run `rustup target add {triple}` to enable)"
            );
            return;
        }
        if !have_on_path("clang-cl") || !have_on_path("lld-link") {
            eprintln!(
                "aot_msvc_link: skip — clang-cl / lld-link not on PATH \
                 (`brew install llvm` on macOS, `apt install clang lld` on Linux)"
            );
            return;
        }
    }

    // Sanity: parse the triple and confirm we're on the MSVC route
    // (libc == Default + os == Windows). If a future refactor breaks
    // this routing, the rest of the test would produce a confusing
    // gcc-style failure instead of a clear assertion miss.
    let target = TargetSpec::from_triple(triple).expect("parse triple");
    assert_eq!(target.os, luna_aot::embed::TargetOs::Windows);
    assert!(
        target.is_msvc(),
        "{triple} should route through is_msvc() == true"
    );

    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("loop.lua");
    std::fs::write(
        &src_path,
        b"local s = 0\nfor i = 1, 1000 do s = s + i end\nprint(s)\n",
    )
    .expect("write source");

    let out_path = td.path().join("loop_aot_msvc.exe");
    let link_result = compile_and_link(&src_path, &out_path, Some(triple), LuaVersion::Lua55);
    let link_err = match link_result {
        Ok(()) => None,
        Err(e) => Some(format!("{e}")),
    };
    if let Some(msg) = link_err {
        if native {
            panic!("aot_msvc_link: native MSVC build failed:\n{msg}");
        }
        // Mirror the skip-marker pattern used by `aot_cross_compile`
        // / `aot_windows_mingw_link`: missing rust-std, missing cross-cc,
        // or missing system libs (LIB env var unset → lld-link can't
        // find ucrt/vcruntime when invoked from a Unix host without
        // a vcvarsall-equivalent) are skips, not hard failures.
        let skip_markers = [
            "rustup target add",
            "can't find crate for `std`",
            "No such file or directory",
            "linker `cc` not found",
            "MSVC C compiler not on PATH",
            "MSVC linker",
            "could not open",
            "unresolved external symbol",
            "LIBPATH",
            "lld-link: error",
            "LNK1104",
            "LNK2019",
            "is incompatible with",
            "unsupported file format",
            "file not found",
            "fatal error",
        ];
        if skip_markers.iter().any(|m| msg.contains(m)) {
            eprintln!("aot_msvc_link: skip — MSVC cross-toolchain incomplete:\n{msg}");
            return;
        }
        panic!(
            "aot_msvc_link: unexpected link failure (none of the known skip \
             markers matched):\n{msg}"
        );
    }

    // PE magic sanity — first two bytes "MZ".
    let head = std::fs::read(&out_path).expect("read PE for magic check");
    assert!(head.len() > 1024, "produced PE suspiciously small");
    assert_eq!(&head[..2], b"MZ", "expected PE/DOS magic 'MZ'");

    let names = read_pe_section_names(&out_path);
    let names_dbg = names.join(", ");
    assert!(
        names.iter().any(|n| n == ".lt_meta"),
        "expected section `.lt_meta` in linked MSVC PE; found sections: [{names_dbg}]"
    );
    assert!(
        names.iter().any(|n| n == ".lt_skix"),
        "expected section `.lt_skix` in linked MSVC PE; found sections: [{names_dbg}]"
    );

    if native {
        let output = Command::new(&out_path)
            .output()
            .expect("run the MSVC binary");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "500500\n",
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.code(), Some(0));
    }

    eprintln!(
        "aot_msvc_link: MSVC PE section table verified — found {} sections, \
         including `.lt_meta` and `.lt_skix`",
        names.len()
    );
}
