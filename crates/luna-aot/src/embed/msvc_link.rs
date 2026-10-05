//! Linking an AOT binary with an MSVC-style linker (`link.exe` / `lld-link`).

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use object::Architecture;

use super::AotError;
use super::msvc_sysroot::MsvcSysroot;
use super::target::{TargetSpec, which_on_path};

impl TargetSpec {
    /// The MSVC-style C compiler driver, or `None` when there is none.
    ///
    /// 1. `$CC` wins.
    /// 2. With `LUNA_AOT_MSVC_SYSROOT` set, `clang-cl` on `PATH` with that
    ///    sysroot's headers; Visual Studio is not looked for.
    /// 3. On a Windows host, `cl.exe` from the newest Visual Studio /
    ///    Build Tools install, with the `INCLUDE` / `LIB` / `PATH` it
    ///    needs set on the command, so no Developer Command Prompt is
    ///    needed (inside one, its environment is used as is).
    /// 4. `clang-cl` on `PATH`, with the headers of the sysroot `cargo
    ///    xwin` keeps in its cache when there is one.
    pub(super) fn msvc_cc_command(&self) -> Result<Option<Command>, AotError> {
        if let Some(cc) = std::env::var_os("CC") {
            return Ok(Some(Command::new(cc)));
        }
        Ok(self.msvc_tools()?.map(|tools| tools.cc))
    }

    /// The MSVC-style PE/COFF linker, or `None` when there is none:
    /// `$LD`, then the same choice as [`Self::msvc_cc_command`] with
    /// `link.exe` and `lld-link` in place of the compilers. A bare `link`
    /// on `PATH` is never taken: on Unix and in Git for Windows' shell
    /// that is the coreutils hard-link tool.
    pub(super) fn msvc_link_command(&self) -> Result<Option<Command>, AotError> {
        if let Some(ld) = std::env::var_os("LD") {
            return Ok(Some(Command::new(ld)));
        }
        Ok(self.msvc_tools()?.map(|tools| tools.link))
    }

    fn msvc_tools(&self) -> Result<Option<MsvcTools>, AotError> {
        let explicit = MsvcSysroot::explicit()?;
        if explicit.is_none()
            && let (Some(cc), Some(link)) = (
                visual_studio_tool(&self.triple, "cl.exe"),
                visual_studio_tool(&self.triple, "link.exe"),
            )
        {
            return Ok(Some(MsvcTools {
                cc,
                link,
                sysroot: None,
            }));
        }
        if !which_on_path("clang-cl") || !which_on_path("lld-link") {
            return Ok(None);
        }
        let sysroot = explicit.or_else(MsvcSysroot::cargo_xwin_cache);
        let mut cc = Command::new("clang-cl");
        let mut link = Command::new("lld-link");
        if let Some(s) = &sysroot {
            cc.args(s.cc_args());
            link.args(s.link_args(self.arch));
        }
        Ok(Some(MsvcTools { cc, link, sysroot }))
    }

    /// Environment for the cargo build of the runtime-helpers staticlib,
    /// whose build script compiles C through the `cc` crate: when luna-aot
    /// links with LLVM's tools, `cc` gets `clang-cl` and the same headers
    /// (it finds `llvm-lib` next to `clang-cl` itself). Empty otherwise,
    /// and for each variable the user already set.
    pub(super) fn staticlib_cc_env(&self) -> Result<Vec<(String, OsString)>, AotError> {
        if !self.is_msvc() || std::env::var_os("CC").is_some() {
            return Ok(Vec::new());
        }
        let Some(MsvcTools {
            sysroot: Some(sysroot),
            ..
        }) = self.msvc_tools()?
        else {
            return Ok(Vec::new());
        };
        let key = self.triple.replace('-', "_");
        let mut flags = OsString::from(format!("--target={}", self.triple));
        for arg in sysroot.cc_args() {
            if arg.to_string_lossy().contains(char::is_whitespace) {
                return Err(AotError::Link(format!(
                    "the MSVC sysroot path in `{}` contains whitespace, which \
                     the `cc` crate's CFLAGS cannot carry; move the sysroot to \
                     a path without spaces",
                    arg.to_string_lossy()
                )));
            }
            flags.push(" ");
            flags.push(arg);
        }
        let mut env = Vec::new();
        for (name, value) in [
            (format!("CC_{key}"), OsString::from("clang-cl")),
            (format!("CFLAGS_{key}"), flags),
        ] {
            if std::env::var_os(&name).is_none() {
                env.push((name, value));
            }
        }
        Ok(env)
    }
}

/// The compiler and linker luna-aot drives for an MSVC target.
struct MsvcTools {
    cc: Command,
    link: Command,
    /// Set when the tools are LLVM's and a sysroot supplies the CRT and SDK.
    sysroot: Option<MsvcSysroot>,
}

#[cfg(windows)]
fn visual_studio_tool(triple: &str, tool: &str) -> Option<Command> {
    find_msvc_tools::find(triple, tool)
}

#[cfg(not(windows))]
fn visual_studio_tool(_triple: &str, _tool: &str) -> Option<Command> {
    None
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
/// it found. `lld-link` gets the library directories of the sysroot
/// [`TargetSpec::msvc_link_command`] found; without one it reads `LIB`, and
/// on a Windows host also finds an installed MSVC and Windows SDK itself.
pub(super) fn link_aot_binary_msvc(
    bytecode_obj: &Path,
    cmain_obj: &Path,
    traces_obj: Option<&Path>,
    staticlib: &Path,
    out_path: &Path,
    target: &TargetSpec,
) -> Result<(), AotError> {
    let Some(mut cmd) = target.msvc_link_command()? else {
        return Err(AotError::Link(format!(
            "no MSVC linker found for target {} — on a Windows host, \
             install Visual Studio or the Build Tools with the \"Desktop \
             development with C++\" workload (`link.exe` is found without \
             a Developer Command Prompt); on any host, LLVM's `clang-cl` and \
             `lld-link` on PATH with an `xwin splat` sysroot named by \
             LUNA_AOT_MSVC_SYSROOT (or left in cargo-xwin's cache) also \
             work. Override with `LD=...` to point at a custom linker.",
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
