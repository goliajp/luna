//! Stage 4-6 of the AOT pipeline (audit `.dev/rfcs/v1.3-audit-luna-aot.md`):
//! parse + compile Lua source → dump luna bytecode → embed into an
//! object file's `.luna.bytecode` data section → link.
//!
//! This module is the **scaffold cut** of the AOT pipeline. The
//! Cranelift trace-codegen path (Stages 3-4 in the audit) is **not**
//! wired here; it lands in follow-up sessions. Today's flow ends after
//! the bytecode is in a `.luna.bytecode` section bracketed by the two
//! public symbols [`crate::BYTECODE_START_SYMBOL`] and
//! [`crate::BYTECODE_END_SYMBOL`].
//!
//! # Pipeline today
//!
//! ```text
//!   foo.lua
//!     │  luna_core::frontend::parser::parse
//!     ▼
//!   Chunk (AST)
//!     │  luna_core::compiler::compile_chunk
//!     ▼
//!   Gc<Proto> (bytecode tree)
//!     │  luna_core::vm::dump::dump
//!     ▼
//!   Vec<u8>   ── luna body, "\x1bLua" + dialect header + "\x00LunaV1\x00" sentinel + body
//!     │  object::write::Object  (this module)
//!     ▼
//!   foo.luna_bytecode.o   (ELF / Mach-O / PE — host triple)
//!     │  cc foo.luna_bytecode.o entry_stub.o -o foo
//!     ▼
//!   foo   (native binary; today: scaffold entry prints the section size)
//! ```
//!
//! # Follow-up
//!
//! - Wire the [`crate::runtime_stub::aot_main`] into the link step so
//!   the produced binary runs the embedded bytecode through a real
//!   `Vm`. Needs a `luna-core` staticlib per target triple OR a
//!   tempdir-cargo bootstrap (audit § Open question 3 Option A).
//! - Stage 3 refactor: lift the JIT lowerer over `cranelift_module::Module`
//!   so the same code drives `JITModule` (luna-jit) and `ObjectModule`
//!   (luna-aot). Then this module emits a second object containing
//!   Cranelift-lowered trace mcode + symbol exports keyed on
//!   `(Proto*, entry-pc)`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use object::write::{Object, Symbol, SymbolSection};
use object::{Architecture, BinaryFormat, Endianness, SectionKind, SymbolKind, SymbolScope};

use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::runtime::Heap;
use luna_core::version::LuaVersion;
use luna_core::vm::dump;

use crate::{BYTECODE_END_SYMBOL, BYTECODE_SECTION_NAME, BYTECODE_START_SYMBOL};

/// Errors surfaced by the AOT pipeline. Variants intentionally carry
/// the upstream message verbatim so the CLI can pass it through to
/// `stderr` without re-formatting (and so future structured-error
/// consumers can match on the variant tag).
#[derive(Debug)]
pub enum AotError {
    /// Reading the Lua source file failed (missing, permission, ...).
    Io(io::Error),
    /// Parser or compiler rejected the source (PUC-style line/message).
    Syntax(String),
    /// Object-file emission failed (unsupported target triple,
    /// internal `object`-crate error).
    Object(String),
    /// Linker (`cc` / user-supplied driver) failed. Carries the
    /// linker's stderr verbatim so users can diagnose toolchain
    /// issues without re-running.
    Link(String),
    /// The target triple isn't supported by the scaffold yet. Today
    /// this fires for anything other than the host triple — Stage 6
    /// of the audit wires cross-compile via cranelift-codegen +
    /// per-triple `cc` flags.
    UnsupportedTarget(String),
}

impl std::fmt::Display for AotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AotError::Io(e) => write!(f, "io error: {e}"),
            AotError::Syntax(msg) => write!(f, "syntax error: {msg}"),
            AotError::Object(msg) => write!(f, "object-file emission failed: {msg}"),
            AotError::Link(msg) => write!(f, "linker failed: {msg}"),
            AotError::UnsupportedTarget(t) => {
                write!(f, "unsupported target triple in scaffold session: {t}")
            }
        }
    }
}

impl std::error::Error for AotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AotError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for AotError {
    fn from(e: io::Error) -> Self {
        AotError::Io(e)
    }
}

/// Compile `source_path` into `out_path` (a native binary embedding
/// the dumped luna bytecode in a `.luna.bytecode` section).
///
/// `target_triple` is parsed only for the host-vs-cross check; the
/// scaffold session only supports the host triple. Pass `None` to
/// default to the host.
///
/// `version` selects the Lua dialect for parsing + bytecode emit
/// (defaults to [`LuaVersion::Lua55`] when called via the CLI).
///
/// # End-to-end behaviour today
///
/// The produced binary is **runnable**: it prints the embedded
/// bytecode length to `stderr` and exits 0. Wiring it to construct a
/// `Vm` and execute the bytecode (the real "interp-only AOT" goal of
/// the audit) is the next follow-up; the runtime-side code already
/// lives in [`crate::runtime_stub`] and compiles cleanly, it just
/// needs to be linked in (audit § Stage 6 Option A — cargo bootstrap
/// of a tiny `luna-core`-depending crate).
pub fn embed_bytecode(
    source_path: &Path,
    out_path: &Path,
    target_triple: Option<&str>,
    version: LuaVersion,
) -> Result<(), AotError> {
    if let Some(t) = target_triple {
        if t != host_triple() {
            return Err(AotError::UnsupportedTarget(t.to_string()));
        }
    }

    // Stage 1 + 2: source → AST → Proto. Uses the same path the runtime
    // `Vm::load` walks (`luna-core/src/vm/exec.rs:1298`).
    let src = fs::read(source_path)?;
    let ast = parse(&src, version).map_err(|e| {
        AotError::Syntax(format!(
            "{}:{}: {}",
            source_path.display(),
            e.line,
            String::from_utf8_lossy(&e.msg)
        ))
    })?;

    // Build-host Heap. Only used for compile_chunk's interning; we
    // drop it after dumping (the Proto's interned strings are
    // serialised into the dump bytes, the heap itself isn't needed
    // beyond this scope).
    let mut heap = Heap::new();
    let chunk_name = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("aot-chunk")
        .as_bytes()
        .to_vec();
    let proto = compile_chunk(&ast, version, &chunk_name, &mut heap).map_err(|e| {
        AotError::Syntax(format!(
            "{}:{}: {}",
            source_path.display(),
            e.line,
            String::from_utf8_lossy(&e.msg)
        ))
    })?;

    // `Gc<T>` implements `Deref<Target = T>`; the `&*proto` reborrow
    // is safe for the same reason `Vm::load` (`exec.rs:1267-1274`)
    // takes the proto reference for `undump` immediately after
    // construction — single-threaded heap, no concurrent mutator.
    let dump_bytes = dump::dump(&proto, false, version);

    // Stage 5: write the bytecode object file.
    let workdir = out_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = out_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("luna_aot");
    let bytecode_obj_path = workdir.join(format!("{stem}.luna_bytecode.o"));
    let stub_obj_path = workdir.join(format!("{stem}.luna_stub.o"));

    write_bytecode_object(&dump_bytes, &bytecode_obj_path)?;
    write_scaffold_entry_object(&stub_obj_path)?;

    // Stage 6: link via system `cc`. The scaffold uses a minimal C
    // entry that references the bracket symbols (proves the section
    // is reachable end-to-end). Follow-up sessions replace this
    // with the Rust `runtime_stub` linked as a staticlib.
    link_with_cc(&[&bytecode_obj_path, &stub_obj_path], out_path)?;

    Ok(())
}

/// Emit `.o` containing a single `.luna.bytecode` data section with
/// the dump bytes, bracketed by two **global** symbols
/// `__luna_bytecode_start` / `__luna_bytecode_end`. Mach-O symbols
/// are prefixed with `_` so the C linker resolves the bare names.
fn write_bytecode_object(bytecode: &[u8], out: &Path) -> Result<(), AotError> {
    let (format, arch, endian) = host_object_target();
    let mut obj = Object::new(format, arch, endian);

    // Single read-only data section. We avoid `StandardSection::ReadOnlyData`
    // (which would land us in `.rodata` / `__DATA,__const`) so the section
    // name is preserved verbatim and `objdump -j .luna.bytecode` finds it.
    let section_id = obj.add_section(
        Vec::new(),
        BYTECODE_SECTION_NAME.as_bytes().to_vec(),
        SectionKind::ReadOnlyData,
    );

    // append section data first, then point the start symbol at offset 0
    let _start_offset = obj.append_section_data(section_id, bytecode, 1);

    // The `object` crate auto-prefixes Mach-O global symbols with `_`
    // per `Mangling::global_prefix` (`object/src/write/mod.rs:391`).
    // We pass the bare name; the output `.o` ends up with the correct
    // per-format mangling.
    let _ = format; // marker for the per-format mangling discussed above
    let start_name = BYTECODE_START_SYMBOL.to_string();
    let end_name = BYTECODE_END_SYMBOL.to_string();

    let _start_sym = obj.add_symbol(Symbol {
        name: start_name.into_bytes(),
        value: 0,
        size: 0,
        kind: SymbolKind::Data,
        // `Dynamic` exposes the symbol as a regular `N_EXT` extern on
        // Mach-O / a global on ELF. `Linkage` would add `N_PEXT` on
        // Mach-O (private extern → `.hidden`), which the static linker
        // can't resolve from another object file.
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(section_id),
        flags: object::SymbolFlags::None,
    });
    let _end_sym = obj.add_symbol(Symbol {
        name: end_name.into_bytes(),
        value: bytecode.len() as u64,
        size: 0,
        kind: SymbolKind::Data,
        // `Dynamic` exposes the symbol as a regular `N_EXT` extern on
        // Mach-O / a global on ELF. `Linkage` would add `N_PEXT` on
        // Mach-O (private extern → `.hidden`), which the static linker
        // can't resolve from another object file.
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(section_id),
        flags: object::SymbolFlags::None,
    });

    let bytes = obj
        .write()
        .map_err(|e| AotError::Object(format!("Object::write: {e}")))?;
    fs::write(out, bytes)?;
    Ok(())
}

/// Emit a tiny C-style entry-point object that references the bytecode
/// bracket symbols and prints the embedded length to stderr.
///
/// This is the **scaffold runtime**. The real runtime —
/// [`crate::runtime_stub::aot_main`] — constructs a `Vm`, calls
/// `Vm::load(&bytecode_slice, b"=embedded")`, and runs it. Wiring
/// that path requires the Rust runtime to be linked into the binary,
/// which is the next follow-up (audit § Stage 6).
fn write_scaffold_entry_object(out: &Path) -> Result<(), AotError> {
    // Generate a C source file in a tempfile, then invoke `cc -c` to
    // produce the `.o`. This is simpler than hand-rolling the entry
    // point in `object` (which would mean writing per-arch assembly
    // for `main`'s ABI).
    let c_src = format!(
        r#"#include <stdio.h>
#include <stdint.h>

extern uint8_t __luna_bytecode_start[];
extern uint8_t __luna_bytecode_end[];

int main(int argc, char **argv) {{
    size_t len = (size_t)(__luna_bytecode_end - __luna_bytecode_start);
    fprintf(stderr,
        "luna-aot scaffold: embedded bytecode length = %zu bytes (section %s)\n"
        "  (interp dispatch wiring is a follow-up session)\n",
        len, "{section}");
    (void)argc; (void)argv;
    return 0;
}}
"#,
        section = BYTECODE_SECTION_NAME
    );

    let mut c_path = out.to_path_buf();
    c_path.set_extension("c");
    fs::write(&c_path, c_src)?;

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let status = Command::new(&cc)
        .arg("-c")
        .arg(&c_path)
        .arg("-o")
        .arg(out)
        .output()
        .map_err(|e| AotError::Link(format!("spawn {cc}: {e}")))?;
    if !status.status.success() {
        return Err(AotError::Link(format!(
            "cc -c {} failed (exit {:?}):\n{}",
            c_path.display(),
            status.status.code(),
            String::from_utf8_lossy(&status.stderr)
        )));
    }
    // best-effort: leave the .c around for diagnosis; future --keep-obj
    // flag controls deletion (audit § CLI surface).
    Ok(())
}

/// Invoke `cc` (or `$CC`) to link object files into `out_path`.
fn link_with_cc(objects: &[&Path], out_path: &Path) -> Result<(), AotError> {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let mut cmd = Command::new(&cc);
    for obj in objects {
        cmd.arg(obj);
    }
    cmd.arg("-o").arg(out_path);
    let output = cmd
        .output()
        .map_err(|e| AotError::Link(format!("spawn {cc}: {e}")))?;
    if !output.status.success() {
        return Err(AotError::Link(format!(
            "cc link failed (exit {:?}):\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

// ────────────────────────────────────────────────────────────────────
// Stage 4 — interp-runtime link path
//
// This is the "real" deploy shape: the produced binary embeds the
// bytecode, links against the `luna-runtime-helpers` staticlib (which
// bundles luna-core + rust stdlib), and runs the embedded chunk
// through a `Vm` at process start.
//
// Pipeline:
//
//   1. Parse + compile + dump (shared with `embed_bytecode`).
//   2. Write `.luna.bytecode` object (shared).
//   3. Build `libluna_runtime_helpers.a` via `cargo build -p
//      luna-runtime-helpers --release` (idempotent — cargo caches).
//   4. Write a tiny C `main.c` that extern-decls the bracket symbols
//      + extern-decls `luna_aot_run`, then calls
//      `luna_aot_run(start, end - start)`. Compile via `cc -c`.
//   5. Link bytecode.o + main.o + libluna_runtime_helpers.a +
//      platform libs (`-lpthread -ldl -lm -framework CoreFoundation`
//      on Mac) into the final binary.
//
// Stage 3 Cranelift trace mcode emission is a separate concern — it
// adds a third object file (containing the lowered traces) to the
// link line. The interp-runtime fallback path lives in the staticlib
// either way, so adding the trace.o is purely additive.
// ────────────────────────────────────────────────────────────────────

/// End-to-end AOT compile: produces a self-contained binary that, when
/// run, loads the embedded bytecode through a luna `Vm` and executes
/// it (interp-driven; Cranelift trace mcode is a follow-up).
///
/// Differs from [`embed_bytecode`]:
/// - Builds and links `luna-runtime-helpers` (staticlib carrying
///   luna-core + a `luna_aot_run` C-ABI entry).
/// - Produced binary actually **runs** the script — `print(...)` lands
///   on stdout, runtime errors print to stderr + exit 1, etc.
///
/// `target_triple` parsing is host-only in this session; cross-compile
/// requires per-triple staticlib builds (`cargo build --target=<triple>
/// -p luna-runtime-helpers`) + the matching `cc --target=...` flag,
/// folded in by Stage 4 cross-compile follow-up.
///
/// `cargo_dir` overrides the working directory `cargo build` runs in
/// (defaults to the workspace this crate lives in, looked up via
/// `CARGO_MANIFEST_DIR`). Useful for downstream embedders that ship
/// a vendored copy of the workspace and want the build to land in a
/// known cache dir.
pub fn compile_and_link(
    source_path: &Path,
    out_path: &Path,
    target_triple: Option<&str>,
    version: LuaVersion,
) -> Result<(), AotError> {
    // Resolve the target: explicit triple if supplied, else host. Anything
    // we can't describe (object-file format, cc invocation, lib set)
    // surfaces as `UnsupportedTarget` here — no silent fallback to host.
    let target = match target_triple {
        Some(t) => TargetSpec::from_triple(t)?,
        None => TargetSpec::host(),
    };

    // Stage 1 + 2 + 5a: shared with `embed_bytecode`.
    let dump_bytes = compile_to_dump(source_path, version)?;

    let workdir = out_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = out_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("luna_aot");
    let bytecode_obj_path = workdir.join(format!("{stem}.luna_bytecode.o"));
    let cmain_obj_path = workdir.join(format!("{stem}.luna_cmain.o"));

    // Stage 5b: bytecode object — target-aware format/arch.
    write_bytecode_object_for(&dump_bytes, &bytecode_obj_path, &target)?;

    // Stage 6a: tiny C main that calls into the staticlib. The C source
    // is target-independent (extern decls only); the `cc -c` invocation
    // routes through the target-aware cc driver so the .o has the right
    // ABI / object-format magic.
    write_aot_cmain_object_for(&cmain_obj_path, &target)?;

    // Stage 6b: ensure the runtime staticlib exists for `target`.
    // For the host triple this is a workspace cargo build; for a cross
    // triple it's `cargo build --target=<triple>` and the resulting
    // staticlib lives under `target/<triple>/release-aot-helpers/`.
    // The `release-aot-helpers` profile (workspace `Cargo.toml`) has
    // `lto = "off"` so the 27 `luna_jit_*` Cranelift trace-mcode
    // helpers survive the rlib → staticlib bundling step.
    let staticlib = build_runtime_helpers_staticlib(target.triple_for_cargo())?;

    // Stage 6c: final link via the target's cc driver. Order matters on
    // some toolchains: bytecode + main first (they reference symbols
    // from the staticlib), then the staticlib, then system libs.
    link_aot_binary_for(
        &bytecode_obj_path,
        &cmain_obj_path,
        &staticlib,
        out_path,
        &target,
    )?;

    Ok(())
}

/// Convenience wrapper for callers that want explicit host-target builds.
pub fn compile_and_link_host(
    source_path: &Path,
    out_path: &Path,
    version: LuaVersion,
) -> Result<(), AotError> {
    compile_and_link(source_path, out_path, None, version)
}

/// Run Stages 1-2 (parse + compile) and produce the dump bytes the
/// bytecode object holds. Factored out so [`embed_bytecode`] and
/// [`compile_and_link`] share the front-end exactly.
fn compile_to_dump(source_path: &Path, version: LuaVersion) -> Result<Vec<u8>, AotError> {
    let src = fs::read(source_path)?;
    let ast = parse(&src, version).map_err(|e| {
        AotError::Syntax(format!(
            "{}:{}: {}",
            source_path.display(),
            e.line,
            String::from_utf8_lossy(&e.msg)
        ))
    })?;

    let mut heap = Heap::new();
    let chunk_name = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("aot-chunk")
        .as_bytes()
        .to_vec();
    let proto = compile_chunk(&ast, version, &chunk_name, &mut heap).map_err(|e| {
        AotError::Syntax(format!(
            "{}:{}: {}",
            source_path.display(),
            e.line,
            String::from_utf8_lossy(&e.msg)
        ))
    })?;

    Ok(dump::dump(&proto, false, version))
}

/// Build `libluna_runtime_helpers.a` for the optional `target_triple`
/// (host build when `None`) and return the path to the produced staticlib.
///
/// Resolution rules:
///
/// 1. If `LUNA_AOT_RUNTIME_HELPERS_STATICLIB` is set, take it as the
///    absolute path of a pre-built `.a` and skip the cargo build.
///    Useful for distribution scenarios where the staticlib is shipped
///    out-of-band (audit § Stage 6 Option B). Only honoured for the
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
fn build_runtime_helpers_staticlib(target_triple: Option<&str>) -> Result<PathBuf, AotError> {
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
        AotError::Link(
            "CARGO_MANIFEST_DIR not set — cannot locate workspace to build \
             luna-runtime-helpers. Set LUNA_AOT_RUNTIME_HELPERS_STATICLIB \
             to bypass."
                .to_string(),
        )
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
        // v1.3 Stage 7 follow-on — dedicated `release-aot-helpers`
        // profile (defined in workspace `Cargo.toml`) turns LTO off
        // for this staticlib build. Workspace `[profile.release]`
        // has `lto = true`, which strips the 27 `luna_jit_*`
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

/// Map the host triple to `object::{BinaryFormat, Architecture, Endianness}`.
/// Scaffold only — cross-compile triples flow in via a richer map in
/// the follow-up Stage 6 work.
fn host_object_target() -> (BinaryFormat, Architecture, Endianness) {
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
fn host_triple() -> &'static str {
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
// Stage 5 — target-aware emission + cross-compile + Windows linker
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
// keeps its `cfg!`-derived defaults so we don't regress the
// already-shipped Stage 4 path.
// ────────────────────────────────────────────────────────────────────

/// Per-target bundle of facts the AOT pipeline needs to emit a
/// runnable binary. Constructed via [`TargetSpec::host`] or
/// [`TargetSpec::from_triple`].
#[derive(Debug, Clone)]
pub struct TargetSpec {
    /// The rustc triple string (e.g. "aarch64-apple-darwin"). For the
    /// host build this matches [`host_triple`]; for cross builds it's
    /// whatever the caller passed via `--target`.
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
    /// Host-triple spec. Mirrors the Stage 4 host-only path: object
    /// format from `cfg!` derivation, libc from `cfg!(target_env)`.
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
        // triple, route through `host()` so we get the Stage-4 lib
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

    /// Pick the `cc` driver invocation for this target. Returns the
    /// command (already constructed with the driver name and any
    /// `-target` / `--target` flags) ready for the caller to add
    /// inputs / outputs / lib flags.
    ///
    /// Resolution order:
    ///
    /// 1. `$CC` environment variable wins, full stop (matches the
    ///    Stage 4 host path).
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

/// Target-aware variant of [`write_bytecode_object`]. Emits a `.o`
/// using the target's format/arch/endian rather than the host's.
fn write_bytecode_object_for(
    bytecode: &[u8],
    out: &Path,
    target: &TargetSpec,
) -> Result<(), AotError> {
    let mut obj = Object::new(target.format, target.arch, target.endian);

    let section_id = obj.add_section(
        Vec::new(),
        BYTECODE_SECTION_NAME.as_bytes().to_vec(),
        SectionKind::ReadOnlyData,
    );
    let _start_offset = obj.append_section_data(section_id, bytecode, 1);

    let start_name = BYTECODE_START_SYMBOL.to_string();
    let end_name = BYTECODE_END_SYMBOL.to_string();
    let _start_sym = obj.add_symbol(Symbol {
        name: start_name.into_bytes(),
        value: 0,
        size: 0,
        kind: SymbolKind::Data,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(section_id),
        flags: object::SymbolFlags::None,
    });
    let _end_sym = obj.add_symbol(Symbol {
        name: end_name.into_bytes(),
        value: bytecode.len() as u64,
        size: 0,
        kind: SymbolKind::Data,
        scope: SymbolScope::Dynamic,
        weak: false,
        section: SymbolSection::Section(section_id),
        flags: object::SymbolFlags::None,
    });

    let bytes = obj
        .write()
        .map_err(|e| AotError::Object(format!("Object::write: {e}")))?;
    fs::write(out, bytes)?;
    Ok(())
}

/// Target-aware variant of [`write_aot_cmain_object`]. Generates the
/// same C source as Stage 4 but invokes the target-specific cc driver
/// so the produced `.o` has the right ABI.
fn write_aot_cmain_object_for(out: &Path, target: &TargetSpec) -> Result<(), AotError> {
    // v1.3 Phase AOT Stage 7 sub-piece 3 — guarantee the
    // `luna_strkey_idx` section exists in the link image even when
    // the binary linked zero AOT trace `.o`s. Without a defining
    // input the bracket symbols `__start_luna_strkey_idx` /
    // `__stop_luna_strkey_idx` (or the Mach-O `section$start$...`
    // equivalents) are undefined and the link fails. Defining an
    // empty placeholder lets the deploy resolver see `start == end`
    // and short-circuit cleanly.
    //
    // The placeholder uses a `static` zero-length array marked
    // `used` so the C compiler emits the section header even though
    // nothing references it. On Mach-O the `section` attribute
    // takes a `"__SEG,__SECT"` pair; we use `__DATA,luna_strkey_idx`
    // mirroring the lowerer's `set_segment_section("", ...)` call
    // (cranelift's empty segment routes to `__DATA` on Mach-O). On
    // ELF the `section` attribute takes just the section name.
    let placeholder = match target.os {
        TargetOs::MacOs => {
            "__attribute__((used, section(\"__DATA,luna_strkey_idx\")))\n\
             static const char luna_strkey_idx_placeholder[1] = {0};\n"
        }
        TargetOs::Linux => {
            "__attribute__((used, section(\"luna_strkey_idx\")))\n\
             static const char luna_strkey_idx_placeholder[1] = {0};\n"
        }
        TargetOs::Windows => "",
    };

    let c_src = format!(
        r#"#include <stddef.h>
#include <stdint.h>

extern uint8_t __luna_bytecode_start[];
extern uint8_t __luna_bytecode_end[];
extern int luna_aot_run(const uint8_t *bytecode, size_t len);

{placeholder}

int main(int argc, char **argv) {{
    (void)argc; (void)argv;
    size_t len = (size_t)(__luna_bytecode_end - __luna_bytecode_start);
    return luna_aot_run(__luna_bytecode_start, len);
}}
"#
    );

    let mut c_path = out.to_path_buf();
    c_path.set_extension("c");
    fs::write(&c_path, c_src)?;

    let mut cmd = target.cc_command();
    cmd.arg("-c").arg(&c_path).arg("-o").arg(out);
    let status = cmd
        .output()
        .map_err(|e| AotError::Link(format!("spawn cc for target {}: {e}", target.triple)))?;
    if !status.status.success() {
        return Err(AotError::Link(format!(
            "cc -c {} (target {}) failed (exit {:?}):\n{}",
            c_path.display(),
            target.triple,
            status.status.code(),
            String::from_utf8_lossy(&status.stderr)
        )));
    }
    Ok(())
}

/// Target-aware variant of [`link_aot_binary`]. Picks the cc driver,
/// per-OS lib set, and (for Windows) the MinGW vs MSVC path.
fn link_aot_binary_for(
    bytecode_obj: &Path,
    cmain_obj: &Path,
    staticlib: &Path,
    out_path: &Path,
    target: &TargetSpec,
) -> Result<(), AotError> {
    // Windows MSVC requires link.exe and a totally different invocation
    // surface (`/OUT:`, `/SUBSYSTEM:CONSOLE`, msvcrt libs). We don't
    // attempt to drive link.exe directly — instead the MSVC arm
    // returns a clear `AotError::Link` directing the user to either
    // (a) build from a Windows host where the staticlib's
    // `--print native-static-libs` output is reliable, or
    // (b) target `x86_64-pc-windows-gnu` (MinGW) which uses a gcc-
    //     style driver we *can* shell out to.
    if target.os == TargetOs::Windows && target.libc == TargetLibc::Default {
        return Err(AotError::Link(format!(
            "MSVC link path (triple {}) is not implemented — luna-aot only \
             drives gcc/clang-style cc drivers. Workarounds: (1) target \
             x86_64-pc-windows-gnu instead (MinGW; works on linux/mac hosts \
             with the matching cross-gcc installed), or (2) run luna-aot \
             with `--scaffold-only` to emit the bytecode object + run \
             `link.exe` by hand.",
            target.triple
        )));
    }

    let mut cmd = target.cc_command();

    // Object files first (they reference symbols defined in the
    // staticlib). Order matters for some traditional Unix linkers
    // (`ld` resolves left-to-right; modern `lld` is order-independent
    // but we keep the canonical order for portability).
    cmd.arg(cmain_obj).arg(bytecode_obj).arg(staticlib);

    // Per-OS lib set — what `rustc --print native-static-libs` reports
    // for a `crate-type = ["staticlib"]` on each platform that pulls
    // std. Match Stage 4's host-only set verbatim for the macOS+linux
    // host paths so the cross arm doesn't regress an already-shipped
    // path.
    match target.os {
        TargetOs::MacOs => {
            cmd.args(["-framework", "CoreFoundation"]);
            cmd.args(["-framework", "Security"]);
            cmd.arg("-liconv");
        }
        TargetOs::Linux => {
            cmd.arg("-lpthread");
            cmd.arg("-ldl");
            cmd.arg("-lm");
            // glibc-only libs; musl ships these symbols inside libc
            // so naming them here would be a `cannot find -lgcc_s` /
            // `cannot find -lutil` failure on Alpine.
            if target.libc != TargetLibc::Musl {
                cmd.arg("-lrt");
                cmd.arg("-lgcc_s");
                cmd.arg("-lutil");
            }
        }
        TargetOs::Windows => {
            // MinGW: rust stdlib's std::sys::windows shim needs these.
            // The set comes from `rustc --print native-static-libs --target=x86_64-pc-windows-gnu`
            // run on a probe staticlib in CI; we replicate the typical
            // dependency list here.
            cmd.arg("-luserenv");
            cmd.arg("-lkernel32");
            cmd.arg("-lws2_32");
            cmd.arg("-lbcrypt");
            cmd.arg("-ladvapi32");
            cmd.arg("-lntdll");
            // MinGW gcc adds its own startup; nothing more needed.
        }
    }

    cmd.arg("-o").arg(out_path);

    let output = cmd
        .output()
        .map_err(|e| AotError::Link(format!("spawn cc for target {}: {e}", target.triple)))?;
    if !output.status.success() {
        return Err(AotError::Link(format!(
            "cc link failed (target {}, exit {:?}):\ncommand: {:?}\nstderr:\n{}",
            target.triple,
            output.status.code(),
            cmd,
            String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(())
}
