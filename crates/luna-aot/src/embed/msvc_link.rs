//! Linking an AOT binary with an MSVC-style linker (`link.exe` / `lld-link`).

use std::path::Path;
use std::process::Command;

use object::Architecture;

use super::AotError;
use super::target::{TargetSpec, which_on_path};

impl TargetSpec {
    /// The MSVC-style C compiler driver, or `None` when there is none.
    ///
    /// 1. `$CC` wins.
    /// 2. On a Windows host, `cl.exe` from the newest Visual Studio /
    ///    Build Tools install, with the `INCLUDE` / `LIB` / `PATH` it
    ///    needs set on the command, so no Developer Command Prompt is
    ///    needed (inside one, its environment is used as is).
    /// 3. `clang-cl` on `PATH`: LLVM's driver, which also runs on a
    ///    Unix host for a cross build.
    pub(super) fn msvc_cc_command(&self) -> Option<Command> {
        if let Some(cc) = std::env::var_os("CC") {
            return Some(Command::new(cc));
        }
        visual_studio_tool(&self.triple, "cl.exe").or_else(|| path_tool("clang-cl"))
    }

    /// The MSVC-style PE/COFF linker, or `None` when there is none:
    /// `$LD`, then `link.exe` from Visual Studio on a Windows host
    /// (environment set up as for [`Self::msvc_cc_command`]), then
    /// `lld-link` on `PATH`. A bare `link` on `PATH` is never taken:
    /// on Unix and in Git for Windows' shell that is the coreutils
    /// hard-link tool.
    pub(super) fn msvc_link_command(&self) -> Option<Command> {
        if let Some(ld) = std::env::var_os("LD") {
            return Some(Command::new(ld));
        }
        visual_studio_tool(&self.triple, "link.exe").or_else(|| path_tool("lld-link"))
    }
}

#[cfg(windows)]
fn visual_studio_tool(triple: &str, tool: &str) -> Option<Command> {
    find_msvc_tools::find(triple, tool)
}

#[cfg(not(windows))]
fn visual_studio_tool(_triple: &str, _tool: &str) -> Option<Command> {
    None
}

fn path_tool(name: &str) -> Option<Command> {
    which_on_path(name).then(|| Command::new(name))
}

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
/// `link.exe` resolves system libs through `LIB`, which
/// [`TargetSpec::msvc_link_command`] sets from the Visual Studio install
/// it found. `lld-link` on a Windows host finds the MSVC and Windows SDK
/// libraries on its own; on a Unix host it needs them given through
/// `LIB` (an `xwin`-style splat), or the link fails on `ucrt.lib`.
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
            "no MSVC linker found for target {} — on a Windows host, \
             install Visual Studio or the Build Tools with the \"Desktop \
             development with C++\" workload (`link.exe` is found without \
             a Developer Command Prompt); on any host, LLVM's `lld-link` \
             on PATH also works. Override with `LD=...` to point at a \
             custom linker.",
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
    // an incremental link pads sections for later patching, and the
    // deploy walker reads `.lt_*` as packed arrays of entries
    cmd.arg("/INCREMENTAL:NO");
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
