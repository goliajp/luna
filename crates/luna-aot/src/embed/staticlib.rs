//! Building the `luna-runtime-helpers` staticlib an AOT binary links against.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::AotError;

/// Build `libluna_runtime_helpers.a` for the optional `target_triple`
/// (host build when `None`) and return the path to the produced staticlib.
///
/// Resolution rules:
///
/// 1. If `LUNA_AOT_RUNTIME_HELPERS_STATICLIB` is set, take it as the
///    absolute path of a pre-built `.a` and skip the cargo build.
///    Useful for distribution scenarios where the staticlib is shipped
///    out-of-band. Only honoured for the
///    host triple — cross triples must build their own staticlib so
///    the override doesn't accidentally mix ABIs.
/// 2. Otherwise, look up `CARGO_MANIFEST_DIR`, ascend to the workspace
///    root (two `..`), and invoke
///    `cargo build -p luna-runtime-helpers --profile=release-aot-helpers [--target T]`.
///    The staticlib lands at
///    `target/<T or default>/release-aot-helpers/libluna_runtime_helpers.a`.
///    The dedicated profile (workspace `Cargo.toml`) turns LTO off so
///    the `luna_jit_*` helper symbols survive bundling — see
///    `[profile.release-aot-helpers]` for the rationale.
///
/// Cross-target builds require the matching `rustup target add <triple>`
/// to have been run beforehand; failures (missing rust-std) are
/// reported via the cargo stderr with a helpful hint.
///
/// The `cargo` invocation is idempotent — cargo caches across runs.
/// On a clean workspace the first call takes ~3s; subsequent calls
/// are sub-second.
pub(super) fn build_runtime_helpers_staticlib(
    target_triple: Option<&str>,
) -> Result<PathBuf, AotError> {
    if target_triple.is_none() {
        // Honour the override only for host builds — for cross we
        // must control the ABI to match the linker invocation.
        if let Ok(prebuilt) = std::env::var("LUNA_AOT_RUNTIME_HELPERS_STATICLIB") {
            let p = PathBuf::from(prebuilt);
            if !p.exists() {
                return Err(AotError::Link(format!(
                    "LUNA_AOT_RUNTIME_HELPERS_STATICLIB points at {} but the file does not exist",
                    p.display()
                )));
            }
            return Ok(p);
        }
    }

    // Serialize concurrent in-process callers (e.g. cargo's parallel
    // integration tests). Cargo's own target-dir lock handles
    // inter-process serialization, but in-process callers can race
    // each other against cargo's "file briefly absent during atomic
    // rename" window. A `Mutex` collapses that to one sequential
    // build at a time per process.
    use std::sync::Mutex;
    static BUILD_LOCK: Mutex<()> = Mutex::new(());
    let _guard = BUILD_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // `CARGO_MANIFEST_DIR` is the directory containing the
    // **luna-aot** Cargo.toml. The workspace root is two levels up.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| {
        // The advice has to differ by build kind. Suggesting the env-var
        // override for a cross target is actively misleading: the check at
        // the top of this function honours it only when `target_triple`
        // is None, so a user following that hint would set it, see the
        // identical error, and have nothing left to try.
        AotError::Link(match target_triple {
            None => "CARGO_MANIFEST_DIR not set — cannot locate workspace to \
                     build luna-runtime-helpers. Set \
                     LUNA_AOT_RUNTIME_HELPERS_STATICLIB to a prebuilt \
                     staticlib to bypass."
                .to_string(),
            Some(t) => format!(
                "CARGO_MANIFEST_DIR not set — cannot locate workspace to build \
                 luna-runtime-helpers for target {t}. Cross-compilation \
                 requires running luna-aot from inside its workspace (e.g. \
                 `cargo run -p luna-aot -- compile ...`), because the \
                 staticlib must be built for the target ABI. \
                 LUNA_AOT_RUNTIME_HELPERS_STATICLIB does NOT apply here — it \
                 is honoured for host builds only, so that a cross link is \
                 never handed a staticlib built for the wrong ABI."
            ),
        })
    })?;
    let workspace_root = PathBuf::from(&manifest_dir)
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            AotError::Link(format!(
                "could not derive workspace root from CARGO_MANIFEST_DIR={manifest_dir}"
            ))
        })?;

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(&workspace_root)
        .arg("build")
        .arg("-p")
        .arg("luna-runtime-helpers")
        // Dedicated `release-aot-helpers` profile (defined in
        // workspace `Cargo.toml`) turns LTO off
        // for this staticlib build. Workspace `[profile.release]`
        // has `lto = true`, which strips the 39 `luna_jit_*`
        // Cranelift trace-mcode helper symbols from the staticlib
        // bundle (the cross-crate optimizer correctly observes they
        // are never *called* from the staticlib's Rust-side surface
        // and treats their cgus as unreachable). The AOT-binary
        // link step then fails with "undefined reference to
        // `_luna_jit_table_get_field`" et al. for any trace mcode
        // that touches a table.
        //
        // Trade-off: no cross-crate inlining into the helper bodies.
        // Cranelift emits the calls as `Linkage::Import` indirect
        // jumps regardless, so the inlining wouldn't apply at the
        // call site anyway — the runtime-JIT path is unaffected.
        .arg("--profile=release-aot-helpers")
        // Don't inherit RUSTFLAGS that might pollute the staticlib
        // (e.g. coverage instrumentation from the parent test build).
        // Acceptable since the staticlib build is deterministic
        // independent of the parent crate's profile.
        .env_remove("RUSTFLAGS");
    if let Some(t) = target_triple {
        cmd.arg("--target").arg(t);
    }
    let output = cmd
        .output()
        .map_err(|e| AotError::Link(format!("spawn cargo: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Detect the most common cross-compile failure mode (missing
        // rust-std for the target) and translate to a concrete fix.
        let hint = if let Some(t) = target_triple {
            if stderr.contains("can't find crate for `std`")
                || stderr.contains("the `std` crate is not available")
                || stderr.contains("target may not be installed")
            {
                format!(
                    "\nhint: cross-compiling to {t} requires the rust-std component — \
                     run `rustup target add {t}` and retry."
                )
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        return Err(AotError::Link(format!(
            "cargo build -p luna-runtime-helpers{target_suffix} failed (exit {:?}):\nstdout:\n{}\nstderr:\n{}{hint}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            stderr,
            target_suffix = target_triple
                .map(|t| format!(" --target={t}"))
                .unwrap_or_default(),
        )));
    }

    let mut staticlib = workspace_root.join("target");
    if let Some(t) = target_triple {
        staticlib.push(t);
    }
    // Profile name mirrors the `--profile=release-aot-helpers` arg
    // above. Cargo's target dir layout uses the profile name verbatim
    // for non-`dev`/`release` profiles.
    staticlib.push("release-aot-helpers");
    // On Windows the staticlib is named `luna_runtime_helpers.lib`
    // rather than `lib*.a`. We try both so the same code path covers
    // both ABIs.
    let unix_name = "libluna_runtime_helpers.a";
    let windows_name = "luna_runtime_helpers.lib";
    let unix_path = staticlib.join(unix_name);
    let windows_path = staticlib.join(windows_name);
    if unix_path.exists() {
        Ok(unix_path)
    } else if windows_path.exists() {
        Ok(windows_path)
    } else {
        Err(AotError::Link(format!(
            "cargo build succeeded but neither {} nor {} exist",
            unix_path.display(),
            windows_path.display(),
        )))
    }
}
