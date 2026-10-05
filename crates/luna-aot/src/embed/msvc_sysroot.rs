//! A Windows SDK + MSVC CRT tree for LLVM's `clang-cl` / `lld-link`,
//! as `xwin` lays it out, for hosts without Visual Studio.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use object::Architecture;

use super::AotError;

/// The environment variable that names a sysroot explicitly.
pub(super) const SYSROOT_ENV: &str = "LUNA_AOT_MSVC_SYSROOT";

/// A directory holding the MSVC CRT and Windows SDK headers and import
/// libraries, in one of the two layouts `xwin splat` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MsvcSysroot {
    /// `xwin splat`: `crt/{include,lib/<arch>}`,
    /// `sdk/include/{ucrt,um,shared}`, `sdk/lib/{ucrt,um}/<arch>`.
    /// This is also what `cargo xwin` keeps in its cache.
    Splat(PathBuf),
    /// `xwin splat --use-winsysroot-style`: the Visual Studio and Windows
    /// Kits directory tree, which `clang-cl` and `lld-link` read
    /// themselves through `/winsysroot`.
    WinSysroot(PathBuf),
}

impl MsvcSysroot {
    /// The sysroot `LUNA_AOT_MSVC_SYSROOT` names, if it is set. A
    /// directory in neither layout is an error, not a silent fallback.
    pub(super) fn explicit() -> Result<Option<Self>, AotError> {
        let Some(dir) = std::env::var_os(SYSROOT_ENV) else {
            return Ok(None);
        };
        let dir = PathBuf::from(dir);
        Self::at(&dir).map(Some).ok_or_else(|| {
            AotError::Link(format!(
                "{SYSROOT_ENV}={} holds neither an `xwin splat` tree \
                 (crt/include) nor a winsysroot-style tree (VC/Tools/MSVC)",
                dir.display()
            ))
        })
    }

    /// The sysroot `cargo xwin` left in its cache directory, if any.
    pub(super) fn cargo_xwin_cache() -> Option<Self> {
        cache_dir()
            .map(|d| d.join("cargo-xwin").join("xwin"))
            .and_then(|d| Self::at(&d))
    }

    fn at(dir: &Path) -> Option<Self> {
        if dir.join("crt").join("include").is_dir() {
            Some(Self::Splat(dir.to_path_buf()))
        } else if dir.join("VC").join("Tools").join("MSVC").is_dir() {
            Some(Self::WinSysroot(dir.to_path_buf()))
        } else {
            None
        }
    }

    /// `clang-cl` arguments that put the CRT and SDK headers on the
    /// system include path.
    pub(super) fn cc_args(&self) -> Vec<OsString> {
        match self {
            Self::Splat(root) => [
                root.join("crt").join("include"),
                root.join("sdk").join("include").join("ucrt"),
                root.join("sdk").join("include").join("um"),
                root.join("sdk").join("include").join("shared"),
            ]
            .iter()
            .map(|d| joined("/imsvc", d))
            .collect(),
            Self::WinSysroot(root) => vec![joined("/winsysroot", root)],
        }
    }

    /// `lld-link` arguments that put the CRT and SDK import libraries for
    /// `arch` on the library search path.
    pub(super) fn link_args(&self, arch: Architecture) -> Vec<OsString> {
        match self {
            Self::Splat(root) => {
                let a = match arch {
                    Architecture::I386 => "x86",
                    Architecture::Aarch64 => "aarch64",
                    _ => "x86_64",
                };
                [
                    root.join("crt").join("lib").join(a),
                    root.join("sdk").join("lib").join("um").join(a),
                    root.join("sdk").join("lib").join("ucrt").join(a),
                ]
                .iter()
                .map(|d| joined("/LIBPATH:", d))
                .collect()
            }
            Self::WinSysroot(root) => vec![joined("/winsysroot:", root)],
        }
    }
}

fn joined(flag: &str, path: &Path) -> OsString {
    let mut s = OsString::from(flag);
    s.push(path.as_os_str());
    s
}

/// The per-user cache directory, as the `dirs` crate (and so `cargo
/// xwin`) resolves it.
fn cache_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_vendor = "apple") {
        var("HOME").map(|h| h.join("Library").join("Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|h| h.join(".cache")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_told_apart() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(MsvcSysroot::at(td.path()), None);
        std::fs::create_dir_all(td.path().join("VC/Tools/MSVC")).unwrap();
        assert_eq!(
            MsvcSysroot::at(td.path()),
            Some(MsvcSysroot::WinSysroot(td.path().to_path_buf()))
        );
        std::fs::create_dir_all(td.path().join("crt/include")).unwrap();
        let splat = MsvcSysroot::at(td.path()).unwrap();
        assert_eq!(splat, MsvcSysroot::Splat(td.path().to_path_buf()));
        let libs = splat.link_args(Architecture::Aarch64);
        assert_eq!(libs.len(), 3);
        assert!(libs[0].to_string_lossy().starts_with("/LIBPATH:"));
        assert!(libs[2].to_string_lossy().ends_with("aarch64"));
        assert_eq!(splat.cc_args().len(), 4);
    }
}
