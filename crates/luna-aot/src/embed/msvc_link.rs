//! Linking an AOT binary with an MSVC-style linker (`link.exe` / `lld-link`).

use std::path::Path;

use object::Architecture;

use super::AotError;
use super::target::TargetSpec;

/// MSVC link path. Drives
/// `lld-link` (cross-platform) or `link.exe` (Windows Build Tools)
/// directly rather than going through a gcc-style cc driver.
///
/// Linker invocation shape:
///
/// ```text
/// lld-link /NOLOGO /SUBSYSTEM:CONSOLE /OUT:foo.exe \
///          foo.luna_cmain.o foo.luna_bytecode.o [foo.luna_traces.o] \
///          luna_runtime_helpers.lib \
///          bcrypt.lib userenv.lib ws2_32.lib advapi32.lib \
///          ntdll.lib kernel32.lib legacy_stdio_definitions.lib
/// ```
///
/// The system lib set mirrors what `rustc --print native-static-libs
/// --target=x86_64-pc-windows-msvc` reports for a `crate-type =
/// ["staticlib"]` that pulls `std`. `legacy_stdio_definitions.lib` is
/// MSVC-specific (resolves the inline-defined stdio symbols
/// `__imp___stdio_common_vsprintf` etc. that the UCRT headers emit when
/// the host C runtime is the Universal CRT 14.0+).
///
/// `link.exe` resolves system libs via the `LIB` environment variable
/// (set by `vcvarsall.bat`). `lld-link` accepts `/LIBPATH:` flags;
/// when neither LIB nor an explicit path is set, it falls back to
/// system defaults which work on Windows hosts but fail on Unix
/// hosts. This is fine because the staticlib build itself only
/// succeeds when a Windows host or a complete cross-toolchain is
/// present (otherwise we fail earlier in
/// `build_runtime_helpers_staticlib`).
pub(super) fn link_aot_binary_msvc(
    bytecode_obj: &Path,
    cmain_obj: &Path,
    traces_obj: Option<&Path>,
    staticlib: &Path,
    out_path: &Path,
    target: &TargetSpec,
) -> Result<(), AotError> {
    let Some(mut cmd) = target.msvc_link_command() else {
        return Err(AotError::Link(format!(
            "MSVC linker (lld-link / link.exe) not on PATH for target {} — \
             install one of: (a) LLVM (`brew install llvm` on macOS; \
             `apt install lld` on Linux) which ships `lld-link`, or \
             (b) Visual Studio Build Tools 2022 (`link.exe`, Windows host \
             only — invoke luna-aot from a Developer Command Prompt so \
             `PATH` + `LIB` are set). Override with `LD=...` to point at \
             a custom linker.",
            target.triple
        )));
    };

    // Quiet the banner (lld-link and link.exe both accept /NOLOGO).
    cmd.arg("/NOLOGO");
    // Console subsystem — luna-aot binaries are CLI programs.
    cmd.arg("/SUBSYSTEM:CONSOLE");
    // Tell lld-link which PE machine type to emit. link.exe infers from
    // the input objects; lld-link tolerates the flag on both, so we
    // always emit it. Maps from rustc arch component to the PE machine
    // string MSVC link expects.
    let machine = match target.arch {
        Architecture::X86_64 => "X64",
        Architecture::I386 => "X86",
        Architecture::Aarch64 => "ARM64",
        _ => {
            return Err(AotError::Link(format!(
                "MSVC link: unsupported arch {:?} for target {} (supported: \
                 x86_64, i686, aarch64)",
                target.arch, target.triple
            )));
        }
    };
    cmd.arg(format!("/MACHINE:{machine}"));
    cmd.arg(format!("/OUT:{}", out_path.display()));

    // Object files first, then the staticlib. The MSVC linker is
    // section-driven (not order-sensitive like Unix ld), but we keep
    // the canonical order for readability with the MinGW arm above.
    cmd.arg(cmain_obj).arg(bytecode_obj);
    if let Some(traces) = traces_obj {
        cmd.arg(traces);
    }
    cmd.arg(staticlib);

    // System libs the rust stdlib + UCRT pull in. Matches
    // `rustc --print native-static-libs --target=x86_64-pc-windows-msvc`
    // for a staticlib that uses std. MSVC needs the `.lib` suffix
    // (vs MinGW's `-lfoo` short form).
    for lib in &[
        "bcrypt.lib",
        "userenv.lib",
        "ws2_32.lib",
        "advapi32.lib",
        "ntdll.lib",
        "kernel32.lib",
        // Resolves the inline-defined UCRT stdio entry points
        // (`__stdio_common_vsprintf` family). Without this, link
        // fails with `unresolved external symbol __imp___stdio_*`
        // when the staticlib indirectly references printf-family
        // functions.
        "legacy_stdio_definitions.lib",
        // ucrt: the Universal CRT (modern Windows C runtime).
        "ucrt.lib",
        // vcruntime: MSVC's C++ runtime stub (referenced via std even
        // for pure-Rust staticlibs because Rust's panic machinery
        // unwinds through the SEH path).
        "vcruntime.lib",
        // msvcrt: legacy MSVC C runtime. Some std symbols still route
        // here on older UCRT versions; harmless overlap.
        "msvcrt.lib",
    ] {
        cmd.arg(lib);
    }

    let output = cmd.output().map_err(|e| {
        AotError::Link(format!(
            "spawn MSVC linker for target {}: {e}",
            target.triple
        ))
    })?;
    if !output.status.success() {
        return Err(AotError::Link(format!(
            "MSVC link failed (target {}, exit {:?}):\ncommand: {:?}\n\
             stdout:\n{}\nstderr:\n{}",
            target.triple,
            output.status.code(),
            cmd,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(())
}
