//! Per-target facts: object format, cc / linker drivers, Cranelift ISA.

use std::process::Command;

use object::{Architecture, BinaryFormat, Endianness};

use super::AotError;

/// Map the host triple to `object::{BinaryFormat, Architecture, Endianness}`.
/// Used by the scaffold path; cross-compile triples go through
/// [`TargetSpec`].
pub(super) fn host_object_target() -> (BinaryFormat, Architecture, Endianness) {
    let format = BinaryFormat::native_object();
    let arch = if cfg!(target_arch = "x86_64") {
        Architecture::X86_64
    } else if cfg!(target_arch = "aarch64") {
        Architecture::Aarch64
    } else if cfg!(target_arch = "x86") {
        Architecture::I386
    } else if cfg!(target_arch = "riscv64") {
        Architecture::Riscv64
    } else {
        Architecture::Unknown
    };
    // Every tier-1 target object supports is little-endian; if we ever
    // ship s390x or big-endian PPC, this needs a `cfg!(target_endian = "big")`
    // branch.
    let endian = Endianness::Little;
    (format, arch, endian)
}

/// Best-effort host triple guess from compile-time `cfg!`. Only used
/// for the host-vs-cross check; the scaffold rejects anything that
/// doesn't match this string.
pub(super) fn host_triple() -> &'static str {
    // Match the most common rust-toolchain triple spellings. Anything
    // not enumerated falls back to "unknown" so a `--target` that
    // happens to equal "unknown" is still rejected.
    if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        "x86_64-pc-windows-msvc"
    } else {
        "unknown"
    }
}

// ────────────────────────────────────────────────────────────────────
// Target-aware emission + cross-compile + Windows linker
//
// `TargetSpec` is the per-triple bundle of facts the AOT pipeline
// needs:
//
//   - `BinaryFormat` / `Architecture` / `Endianness` so `object::write`
//     emits the right `.o` magic
//   - the `cc` invocation (`cc`, `clang -target ...`, or
//     `<triple>-gcc`) and whether the C entry-point object needs
//     extra flags to land in the right ABI
//   - the lib set the staticlib transitively pulls (libpthread, libm,
//     CoreFoundation, ws2_32, ...) so the final link resolves all
//     externs without leaving the user to figure it out from cargo's
//     `--print native-static-libs` output
//
// Adding a new tier just means a new `from_triple` arm. The host arm
// keeps its `cfg!`-derived defaults so the host path stays
// unchanged.
// ────────────────────────────────────────────────────────────────────

/// Per-target bundle of facts the AOT pipeline needs to emit a
/// runnable binary. Constructed via [`TargetSpec::host`] or
/// [`TargetSpec::from_triple`].
#[derive(Debug, Clone)]
pub struct TargetSpec {
    /// The rustc triple string (e.g. "aarch64-apple-darwin"). For the
    /// host build this matches the value returned by the private
    /// `host_triple` helper; for cross builds it's whatever the caller
    /// passed via `--target`.
    pub triple: String,
    /// `true` when the triple matches the build host's rust triple. The
    /// staticlib-build step shortcuts the `--target` flag in this case,
    /// landing the `.a` under `target/release-aot-helpers/` (the
    /// workspace default dir for the dedicated AOT-helpers profile,
    /// not `target/<triple>/release-aot-helpers/`).
    pub is_host: bool,
    /// Object-file binary format for `object::write::Object::new`.
    pub format: BinaryFormat,
    /// Object-file architecture for `object::write::Object::new`.
    pub arch: Architecture,
    /// Object-file endianness for `object::write::Object::new`. Every
    /// tier-1 cranelift target is little-endian; if we ever ship s390x
    /// or big-endian PPC, this needs to flip.
    pub endian: Endianness,
    /// `cfg!(target_os = "...")` style family for selecting which lib
    /// set to pass at link time. Decoupled from `cc!` so a Linux host
    /// can describe a Windows target without rebuilding luna-aot.
    pub os: TargetOs,
    /// libc flavour — distinguishes glibc vs musl on Linux. Drives
    /// `-lgcc_s` (glibc) vs no-such-lib (musl).
    pub libc: TargetLibc,
}

/// Coarse target-OS family used by [`TargetSpec`]. Granular enough to
/// pick the right `cc` driver and lib set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    /// macOS / Darwin (`*-apple-darwin`).
    MacOs,
    /// Linux (any libc).
    Linux,
    /// Windows (MSVC or MinGW; the libc distinction is on `TargetLibc`).
    Windows,
}

/// libc flavour for the target. Drives the lib set passed at link time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetLibc {
    /// glibc on Linux, Apple libc on Darwin, MSVCRT on Windows MSVC.
    Default,
    /// musl on Linux (Alpine, static deploys). Skips `-lgcc_s` and
    /// `-lutil`, which aren't present in musl's lib set.
    Musl,
    /// MinGW on Windows (gcc-based toolchain producing PE-COFF that
    /// still links against the system C runtime via the mingwex shim).
    MinGw,
}

impl TargetSpec {
    /// Host-triple spec: object format from `cfg!` derivation, libc from `cfg!(target_env)`.
    pub fn host() -> Self {
        let (format, arch, endian) = host_object_target();
        let triple = host_triple().to_string();
        let os = if cfg!(target_os = "macos") {
            TargetOs::MacOs
        } else if cfg!(target_os = "linux") {
            TargetOs::Linux
        } else if cfg!(target_os = "windows") {
            TargetOs::Windows
        } else {
            // Unknown OS: pick Linux as the least-surprising default
            // for unix-y systems; the caller will see a clear cc error
            // if the assumption fails.
            TargetOs::Linux
        };
        let libc = if cfg!(target_env = "musl") {
            TargetLibc::Musl
        } else if cfg!(all(target_os = "windows", target_env = "gnu")) {
            TargetLibc::MinGw
        } else {
            TargetLibc::Default
        };
        TargetSpec {
            triple,
            is_host: true,
            format,
            arch,
            endian,
            os,
            libc,
        }
    }

    /// Parse a triple string into a `TargetSpec`. Unrecognised triples
    /// return [`AotError::UnsupportedTarget`].
    ///
    /// Tier 1 (verified end-to-end on macOS aarch64 host): the host
    /// triple, plus the same-OS cross to `x86_64-apple-darwin` /
    /// `aarch64-apple-darwin` (Apple's universal clang handles both).
    ///
    /// Tier 2 (codegen-verified, link requires the matching cross-cc
    /// toolchain on PATH): `*-unknown-linux-gnu`, `*-unknown-linux-musl`,
    /// `x86_64-pc-windows-gnu` (MinGW). The cargo staticlib build
    /// succeeds when the rust-std for the triple is installed; the
    /// final `cc` link fails with a clear error if the cross-gcc is
    /// missing.
    pub fn from_triple(triple: &str) -> Result<Self, AotError> {
        // Short-circuit: if the requested triple matches the host
        // triple, route through `host()` so we get the host lib
        // detection (which uses the actual cfg! the binary was built
        // under, not the parsed triple string).
        if triple == host_triple() {
            return Ok(Self::host());
        }

        let parts: Vec<&str> = triple.split('-').collect();
        if parts.len() < 3 {
            return Err(AotError::UnsupportedTarget(format!(
                "triple {triple:?} has fewer than 3 components (expected arch-vendor-os[-env])"
            )));
        }
        let arch_str = parts[0];
        // parts[1] is the vendor (unknown / apple / pc / ...); we don't
        // gate on it, only inform the object-format pick via os.
        let os_str = parts[2];
        let env_str = parts.get(3).copied().unwrap_or("");

        let arch = match arch_str {
            "x86_64" => Architecture::X86_64,
            "aarch64" => Architecture::Aarch64,
            "i686" | "i586" | "x86" => Architecture::I386,
            "riscv64" | "riscv64gc" => Architecture::Riscv64,
            other => {
                return Err(AotError::UnsupportedTarget(format!(
                    "arch component {other:?} of triple {triple:?} not in tier 1/2 set \
                     (supported: x86_64, aarch64, i686, riscv64)"
                )));
            }
        };

        let (os, format, libc) = match os_str {
            "darwin" => (TargetOs::MacOs, BinaryFormat::MachO, TargetLibc::Default),
            "linux" => {
                let libc = if env_str.contains("musl") {
                    TargetLibc::Musl
                } else {
                    TargetLibc::Default
                };
                (TargetOs::Linux, BinaryFormat::Elf, libc)
            }
            "windows" => {
                // env can be "gnu" (MinGW) or "msvc"; we route by env.
                let libc = if env_str == "gnu" {
                    TargetLibc::MinGw
                } else {
                    TargetLibc::Default
                };
                (TargetOs::Windows, BinaryFormat::Coff, libc)
            }
            other => {
                return Err(AotError::UnsupportedTarget(format!(
                    "os component {other:?} of triple {triple:?} not in tier 1/2 set \
                     (supported: darwin, linux, windows)"
                )));
            }
        };

        Ok(TargetSpec {
            triple: triple.to_string(),
            is_host: false,
            format,
            arch,
            endian: Endianness::Little,
            os,
            libc,
        })
    }

    /// Cargo `--target` value, or `None` for the host build (cargo
    /// defaults to the host triple when `--target` is omitted).
    pub fn triple_for_cargo(&self) -> Option<&str> {
        if self.is_host {
            None
        } else {
            Some(&self.triple)
        }
    }

    /// `true` when this target uses the MSVC toolchain (Windows + the
    /// default Microsoft libc). Routes `write_aot_cmain_object_for` to
    /// the `clang-cl` / `cl.exe` driver shape and `link_aot_binary_for`
    /// to the `lld-link` / `link.exe` driver shape (vs the gcc-style
    /// `cc -o foo foo.o ...` shape used for every other target).
    pub fn is_msvc(&self) -> bool {
        self.os == TargetOs::Windows && self.libc == TargetLibc::Default
    }

    /// Pick the MSVC-style C compiler driver. Returns `None` when none is on PATH (caller skips with a
    /// clear error message). Resolution:
    ///
    /// 1. `$CC` env var wins (consistent with `cc_command`).
    /// 2. `clang-cl` — cross-platform: macOS/Linux hosts get it via
    ///    `brew install llvm` / `apt install clang`, accepts the same
    ///    `__attribute__((section(...)))` syntax we emit for MinGW.
    /// 3. `cl.exe` — Microsoft Build Tools, Windows-host only. Requires
    ///    `vcvarsall.bat` to have set up INCLUDE / LIB env vars.
    pub(super) fn msvc_cc_command(&self) -> Option<Command> {
        if let Ok(cc) = std::env::var("CC") {
            return Some(Command::new(cc));
        }
        for candidate in &["clang-cl", "cl.exe", "cl"] {
            if which_on_path(candidate) {
                return Some(Command::new(candidate));
            }
        }
        None
    }

    /// Pick the MSVC-style PE/COFF linker driver. Returns `None` when none is on PATH. Resolution:
    ///
    /// 1. `$LD` env var wins (advanced override for embedders shipping a
    ///    pinned linker).
    /// 2. `lld-link` — LLVM's PE/COFF linker. Cross-platform: macOS gets
    ///    it via `brew install llvm`, Linux via `apt install lld`. Works
    ///    without a Windows host or vcvarsall setup.
    /// 3. `link.exe` — Microsoft's linker. Windows-only; requires
    ///    Developer Command Prompt (sets PATH + LIB env vars).
    pub(super) fn msvc_link_command(&self) -> Option<Command> {
        if let Ok(ld) = std::env::var("LD") {
            return Some(Command::new(ld));
        }
        for candidate in &["lld-link", "link.exe", "link"] {
            if which_on_path(candidate) {
                return Some(Command::new(candidate));
            }
        }
        None
    }

    /// Pick the `cc` driver invocation for this target. Returns the
    /// command (already constructed with the driver name and any
    /// `-target` / `--target` flags) ready for the caller to add
    /// inputs / outputs / lib flags.
    ///
    /// Resolution order:
    ///
    /// 1. `$CC` environment variable wins, full stop (matches the
    ///    host path).
    /// 2. For non-host targets we try the toolchain-named cross
    ///    compiler first (e.g. `aarch64-linux-gnu-gcc`,
    ///    `x86_64-w64-mingw32-gcc`, `x86_64-linux-musl-gcc`).
    /// 3. Fall through to `cc -target <triple>` (works on macOS where
    ///    Apple's clang is the system cc and supports cross-darwin
    ///    natively).
    ///
    /// The returned `Command` already has any `-target`/`--target`
    /// flag set; the caller adds the remaining args.
    pub fn cc_command(&self) -> Command {
        if let Ok(cc) = std::env::var("CC") {
            return Command::new(cc);
        }

        if self.is_host {
            return Command::new("cc");
        }

        // Non-host: try the named cross-cc first.
        let cross_candidates: &[&str] = match (self.os, self.arch, self.libc) {
            (TargetOs::Linux, Architecture::Aarch64, TargetLibc::Default) => {
                &["aarch64-linux-gnu-gcc"]
            }
            (TargetOs::Linux, Architecture::X86_64, TargetLibc::Default) => {
                &["x86_64-linux-gnu-gcc"]
            }
            (TargetOs::Linux, Architecture::Aarch64, TargetLibc::Musl) => {
                &["aarch64-linux-musl-gcc", "musl-gcc"]
            }
            (TargetOs::Linux, Architecture::X86_64, TargetLibc::Musl) => {
                &["x86_64-linux-musl-gcc", "musl-gcc"]
            }
            (TargetOs::Windows, Architecture::X86_64, TargetLibc::MinGw) => {
                &["x86_64-w64-mingw32-gcc"]
            }
            (TargetOs::Windows, Architecture::I386, TargetLibc::MinGw) => &["i686-w64-mingw32-gcc"],
            _ => &[],
        };
        for candidate in cross_candidates {
            if which_on_path(candidate) {
                return Command::new(candidate);
            }
        }

        // Apple cross-darwin: clang -target accepts e.g.
        // `x86_64-apple-darwin` directly when the SDK is installed.
        if self.os == TargetOs::MacOs {
            let mut cmd = Command::new("cc");
            cmd.arg("-target").arg(&self.triple);
            return cmd;
        }

        // Last resort: `cc -target` and hope the host cc is clang.
        // Will error at link time on gcc hosts; the error message
        // includes the triple so the user knows what to install.
        let mut cmd = Command::new("cc");
        cmd.arg("-target").arg(&self.triple);
        cmd
    }

    /// Resolve the Cranelift `TargetIsa` builder for this target. Used by
    /// `harvest_and_emit_aot_traces` so the offline trace lowerer
    /// codegens for the deploy ABI rather than the build host's.
    ///
    /// Two layers:
    ///
    /// 1. Parse `self.triple` with `target_lexicon::Triple::from_str`.
    ///    Both Cranelift's `isa::lookup_by_name` and `isa::lookup` go
    ///    through the same path; the explicit `from_str` here surfaces
    ///    a clean `AotError::Object` on malformed triples rather than
    ///    Cranelift's internal `expect` panic.
    /// 2. `cranelift_codegen::isa::lookup(triple)` returns an `isa::
    ///    Builder` configured for the requested arch — provided the
    ///    Cranelift feature for that arch (`x86`, `arm64`, etc.) is
    ///    enabled at compile time. We turn `all-arch` on in
    ///    `Cargo.toml` so any tier-1/2 target resolves. If somebody
    ///    later shrinks the feature set, the `SupportDisabled` arm
    ///    surfaces a clear error.
    ///
    /// On the host triple we still go through `cranelift_native::
    /// builder()` (rather than the per-triple path) so we inherit the
    /// CPU-feature autodetection (`SSE4.1`, `AVX2`, …). Host warmup +
    /// host deploy ⇒ identical mcode.
    // cranelift types in the signature: internal to luna crates, not covered by semver
    #[doc(hidden)]
    pub fn cranelift_isa_builder(&self) -> Result<cranelift_codegen::isa::Builder, AotError> {
        use std::str::FromStr;
        if self.is_host {
            return cranelift_native::builder().map_err(|e| {
                AotError::Object(format!(
                    "cranelift_native::builder for host triple {}: {e}",
                    self.triple
                ))
            });
        }
        let triple = target_lexicon::Triple::from_str(&self.triple).map_err(|e| {
            AotError::Object(format!(
                "target_lexicon could not parse triple {:?}: {e}",
                self.triple
            ))
        })?;
        cranelift_codegen::isa::lookup(triple).map_err(|e| {
            AotError::Object(format!(
                "cranelift_codegen::isa::lookup for triple {}: {e:?} \
                 (re-build luna-aot with `cranelift-codegen` feature \
                 `all-arch` or the per-arch feature for {})",
                self.triple, self.triple,
            ))
        })
    }
}

/// Check whether `binary` resolves on `PATH`. Used by
/// [`TargetSpec::cc_command`] to prefer a named cross-cc over the
/// host `cc`.
fn which_on_path(binary: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(if cfg!(windows) { ';' } else { ':' }) {
            if dir.is_empty() {
                continue;
            }
            let candidate = std::path::Path::new(dir).join(binary);
            if candidate.exists() {
                return true;
            }
        }
    }
    false
}
