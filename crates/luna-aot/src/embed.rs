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
use luna_core::jit::aot_meta::{
    AotTraceMetaHeader, PerExitInlineEntry, PerExitTagsEntry, encode_meta_blob, pack_exit_tag,
    pack_tag_res_kind,
};
use luna_core::jit::trace_types::{CompileOptions, CompiledTrace, TraceRecord};
use luna_core::runtime::Heap;
use luna_core::runtime::Value;
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

    // v1.3 Phase AOT Stage 7 sub-piece 4 (closed) + polish 4
    // (cross-compile traces): offline trace recorder + AOT trace
    // mcode emission. The warmup `Vm` always runs on the **host**
    // (we can't dispatch target mcode at warmup time), but the
    // trace-mcode `.o` we emit is keyed off `TargetSpec`:
    //
    //   - `TargetSpec::cranelift_isa_builder()` resolves the right
    //     Cranelift `TargetIsa` (`x86_64`, `aarch64`, etc.) so the
    //     `ObjectModule` codegens for the deploy ABI, not the host's.
    //   - The recorded `TraceRecord`s are luna-IR-level (op + guard +
    //     reg moves); pointer-width / endianness are encoded as IR
    //     types (`I64` / little-endian) which are stable across every
    //     target in our tier set. So the same `TraceRecord` re-lowers
    //     correctly for any cross target.
    //
    // Returns Ok(None) when no traces close (small / non-loopy source)
    // or the target Cranelift backend isn't compiled in (resolved by
    // the `cranelift-codegen = { features = ["all-arch"] }` dep, but
    // we self-skip rather than panic if it ever shrinks).
    let traces_obj_path = {
        let path = workdir.join(format!("{stem}.luna_traces.o"));
        match harvest_and_emit_aot_traces(&dump_bytes, version, &path, &target)? {
            HarvestedTraces::None => None,
            HarvestedTraces::Some => Some(path),
        }
    };

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
        traces_obj_path.as_deref(),
        &staticlib,
        out_path,
        &target,
    )?;

    Ok(())
}

/// v1.3 Phase AOT Stage 7 sub-piece 4 — return shape for
/// [`harvest_and_emit_aot_traces`]. `None` = warmup recorded zero
/// dispatchable traces (small / non-loopy source); `Some` = at least
/// one trace .o was written.
enum HarvestedTraces {
    /// Warmup didn't produce any dispatchable traces. Pipeline
    /// continues without a trace `.o` on the link line; AOT binary
    /// runs through interp + runtime JIT only.
    None,
    /// Trace `.o` was written at the path passed in. Link step adds it
    /// to the cc invocation; deploy walker installs the contained
    /// traces at startup.
    Some,
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

    /// `true` when this target uses the MSVC toolchain (Windows + the
    /// default Microsoft libc). Routes `write_aot_cmain_object_for` to
    /// the `clang-cl` / `cl.exe` driver shape and `link_aot_binary_for`
    /// to the `lld-link` / `link.exe` driver shape (vs the gcc-style
    /// `cc -o foo foo.o ...` shape used for every other target).
    pub fn is_msvc(&self) -> bool {
        self.os == TargetOs::Windows && self.libc == TargetLibc::Default
    }

    /// v1.3 Phase AOT Stage 7 polish 5 — pick the MSVC-style C compiler
    /// driver. Returns `None` when none is on PATH (caller skips with a
    /// clear error message). Resolution:
    ///
    /// 1. `$CC` env var wins (consistent with `cc_command`).
    /// 2. `clang-cl` — cross-platform: macOS/Linux hosts get it via
    ///    `brew install llvm` / `apt install clang`, accepts the same
    ///    `__attribute__((section(...)))` syntax we emit for MinGW.
    /// 3. `cl.exe` — Microsoft Build Tools, Windows-host only. Requires
    ///    `vcvarsall.bat` to have set up INCLUDE / LIB env vars.
    fn msvc_cc_command(&self) -> Option<Command> {
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

    /// v1.3 Phase AOT Stage 7 polish 5 — pick the MSVC-style PE/COFF
    /// linker driver. Returns `None` when none is on PATH. Resolution:
    ///
    /// 1. `$LD` env var wins (advanced override for embedders shipping a
    ///    pinned linker).
    /// 2. `lld-link` — LLVM's PE/COFF linker. Cross-platform: macOS gets
    ///    it via `brew install llvm`, Linux via `apt install lld`. Works
    ///    without a Windows host or vcvarsall setup.
    /// 3. `link.exe` — Microsoft's linker. Windows-only; requires
    ///    Developer Command Prompt (sets PATH + LIB env vars).
    fn msvc_link_command(&self) -> Option<Command> {
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

    /// v1.3 Phase AOT Stage 7 polish 4 — resolve the Cranelift
    /// `TargetIsa` builder for this target. Used by
    /// [`harvest_and_emit_aot_traces`] so the offline trace lowerer
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
    /// host deploy ⇒ identical mcode, matching the pre-polish-4
    /// behaviour byte-for-byte.
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
    // v1.3 Phase AOT Stage 7 sub-piece 4 — also guarantee the
    // `luna_trace_meta` section exists in the link image when zero
    // trace `.o`s linked in (small / non-loopy sources where the
    // warmup recorder didn't close any traces). Same placeholder
    // pattern as `luna_strkey_idx` from sub-piece 3.
    //
    // **Sized at 48 bytes** (matching `AotTraceIndexEntry::SIZE`) and
    // 8-byte aligned so when a real AOT-emitted trace .o lands in the
    // same section, the linker's section merge concatenates the
    // entries without misaligning anyone. The placeholder bytes are
    // all-zero; the deploy walker sees a single entry whose `fn_ptr`
    // is NULL and skips it (via the `entry.fn_ptr.is_null()` guard).
    //
    // `aligned(8)` is needed because the section's other entries
    // carry pointer relocations at offsets 24 / 32 — a misaligned
    // placeholder shifts those into bad lanes.
    // Strkey idx placeholder: sized to a full `IndexEntry` (16 bytes,
    // 8-byte aligned) so the deploy resolver's
    // `start + N * sizeof::<IndexEntry>` iteration lines up with
    // any real trace-emitted entries that follow. A `[1]`-sized
    // placeholder used to land 7 bytes of zero pad between itself
    // and the first trace entry (the trace lower sets `align(8)`),
    // which mis-aligned the divide-by-16 entry count: the walker
    // saw the placeholder as half an entry and missed the real one
    // by 8 bytes. Sizing the placeholder to 16 means the section
    // is exactly N+1 entries for N real traces; the placeholder's
    // zero-valued `bytes_ptr` short-circuits via the resolver's
    // `entry.bytes_ptr.is_null()` guard.
    // v1.3 Phase AOT Stage 7 polish 6 — also guarantee the
    // `luna_inline_chnx` section exists when the binary linked zero
    // depth>0-inlined-cmp trace `.o`s. Same shape as the strkey idx
    // placeholder (16 bytes = one IndexEntry-sized slot) so the deploy
    // resolver's `start + N * sizeof::<IndexEntry>` walk lines up with
    // any real trace-emitted entries. Zero `bytes_ptr` field short-
    // circuits via the resolver's null guard.
    //
    // Mach-O sectname max is 16 chars; `luna_inline_chnx` is 15
    // (matches `luna_strkey_idx` sizing). Windows COFF short name cap
    // is 8 — `.lt_chai` mirrors `.lt_skix` / `.lt_meta`. Both names
    // must match the lowerer's `set_segment_section` choice in
    // `emit_chain_ptr_arg` and the deploy resolver's bracket /
    // section-walker needles.
    let placeholder = match target.os {
        TargetOs::MacOs => {
            "__attribute__((used, section(\"__DATA,luna_strkey_idx\"), aligned(8)))\n\
             static const char luna_strkey_idx_placeholder[16] = {0};\n\
             __attribute__((used, section(\"__DATA,luna_trace_meta\"), aligned(8)))\n\
             static const char luna_trace_meta_placeholder[48] = {0};\n\
             __attribute__((used, section(\"__DATA,luna_inline_chnx\"), aligned(8)))\n\
             static const char luna_inline_chnx_placeholder[16] = {0};\n"
        }
        TargetOs::Linux => {
            "__attribute__((used, section(\"luna_strkey_idx\"), aligned(8)))\n\
             static const char luna_strkey_idx_placeholder[16] = {0};\n\
             __attribute__((used, section(\"luna_trace_meta\"), aligned(8)))\n\
             static const char luna_trace_meta_placeholder[48] = {0};\n\
             __attribute__((used, section(\"luna_inline_chnx\"), aligned(8)))\n\
             static const char luna_inline_chnx_placeholder[16] = {0};\n"
        }
        // v1.3 Phase AOT Stage 7 polish 3 — Windows COFF.
        //
        // PE/COFF section name headers are fixed 8 bytes
        // (`IMAGE_SECTION_HEADER::Name`), so we use deliberately
        // short names: `.lt_skix` for the strkey index (mirrors
        // `luna_strkey_idx`), `.lt_meta` for trace meta. Each is
        // exactly 8 bytes including the leading `.`, matching the
        // 8-byte PE section name field byte-for-byte without truncation
        // or string-table fallback (the COFF string-table mechanism
        // for long names is an object-file feature only — `link.exe`
        // / `lld-link` drop the long-name table when producing the
        // final PE image).
        //
        // Mirror placeholders so the sections exist even when the
        // binary linked zero AOT trace `.o`s — same shape as the
        // Mach-O / ELF placeholders above. The deploy walker
        // (`luna-runtime-helpers::windows_section::find_section`)
        // sees the placeholder bytes via the runtime PE-header
        // parse and short-circuits on the all-zero entry.
        //
        // MinGW's gcc accepts the `__attribute__((section(...)))`
        // syntax verbatim with the section name as-is (no leading
        // `__DATA,` prefix — that's Mach-O specific). For MSVC we
        // emit the equivalent `#pragma section` + `__declspec(allocate(...))`
        // form so the same placeholder data lands in the same
        // `.lt_skix` / `.lt_meta` sections regardless of toolchain;
        // the deploy walker (`luna-runtime-helpers::windows_section`)
        // looks up by section name and is toolchain-agnostic.
        //
        // clang-cl accepts both syntaxes (`__attribute__((section()))`
        // and the MSVC `__declspec(allocate())` form), but `cl.exe`
        // only accepts the MSVC form — so we emit the MSVC form for
        // both, which keeps a single source path covering both drivers.
        TargetOs::Windows if target.is_msvc() => {
            // `#pragma section` declares the section + its
            // characteristics (R = readable). The 8-byte alignment
            // matches the MinGW arm so the deploy walker's pointer
            // arithmetic over the section is identical across
            // toolchains.
            "#pragma section(\".lt_skix\", read)\n\
             __declspec(allocate(\".lt_skix\")) __declspec(align(8))\n\
             static const char luna_strkey_idx_placeholder[16] = {0};\n\
             #pragma section(\".lt_meta\", read)\n\
             __declspec(allocate(\".lt_meta\")) __declspec(align(8))\n\
             static const char luna_trace_meta_placeholder[48] = {0};\n\
             #pragma section(\".lt_chai\", read)\n\
             __declspec(allocate(\".lt_chai\")) __declspec(align(8))\n\
             static const char luna_inline_chnx_placeholder[16] = {0};\n"
        }
        TargetOs::Windows => {
            "__attribute__((used, section(\".lt_skix\"), aligned(8)))\n\
             static const char luna_strkey_idx_placeholder[16] = {0};\n\
             __attribute__((used, section(\".lt_meta\"), aligned(8)))\n\
             static const char luna_trace_meta_placeholder[48] = {0};\n\
             __attribute__((used, section(\".lt_chai\"), aligned(8)))\n\
             static const char luna_inline_chnx_placeholder[16] = {0};\n"
        }
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

    // v1.3 Phase AOT Stage 7 polish 5 — MSVC needs `clang-cl` / `cl.exe`
    // (different flag shape: `/c` + `/Fo:` vs gcc-style `-c` + `-o`).
    // All other targets keep the existing gcc-style cc driver path.
    let mut cmd = if target.is_msvc() {
        let Some(mut cl) = target.msvc_cc_command() else {
            return Err(AotError::Link(format!(
                "MSVC C compiler not on PATH for target {} — install one of: \
                 (a) `clang-cl` via LLVM (`brew install llvm` on macOS; \
                 `apt install clang` on Linux), or (b) Visual Studio Build \
                 Tools 2022 (`cl.exe`, Windows host only — invoke luna-aot \
                 from a Developer Command Prompt). Override with `CC=...` \
                 to point at a custom driver.",
                target.triple
            )));
        };
        // `clang-cl` / `cl.exe`: `/c` compile-only, `/Fo:<obj>` output.
        // Some Linux distros' clang-cl wrappers also accept gcc-style
        // flags, but the MSVC shape works on every supported driver.
        cl.arg("/c");
        cl.arg(&c_path);
        // `/Fo:` and the output path are a single token when no space —
        // we use the safe two-arg form via `arg(format!("/Fo:{}", ..))`
        // which avoids whitespace-in-path issues.
        cl.arg(format!("/Fo:{}", out.display()));
        // Suppress the cl.exe banner (clang-cl no-ops on this flag).
        cl.arg("/nologo");
        // Cross-compile target: clang-cl accepts `--target=<triple>` to
        // override the default host. cl.exe rejects this; we only set it
        // for clang-cl by detecting the program name (heuristic — first
        // arg defaulted via `Command::new`). The simplest robust path
        // is to always set it when not on a Windows host, since cl.exe
        // can't realistically run there anyway. Skipping for now: cmd
        // here is `clang-cl` only when reachable on a Unix host.
        if !cfg!(target_os = "windows") {
            cl.arg(format!("--target={}", target.triple));
        }
        cl
    } else {
        let mut cmd = target.cc_command();
        cmd.arg("-c").arg(&c_path).arg("-o").arg(out);
        cmd
    };
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
///
/// `traces_obj` (Stage 7 sub-piece 4): optional AOT-trace mcode `.o`
/// emitted by [`harvest_and_emit_aot_traces`]. When `Some`, the linker
/// pulls in the trace mcode + the `luna_trace_meta` / `luna_trace_blob`
/// data sections that the deploy walker reads at startup. When `None`,
/// the binary runs through interp + runtime-JIT fallback only.
fn link_aot_binary_for(
    bytecode_obj: &Path,
    cmain_obj: &Path,
    traces_obj: Option<&Path>,
    staticlib: &Path,
    out_path: &Path,
    target: &TargetSpec,
) -> Result<(), AotError> {
    // v1.3 Phase AOT Stage 7 polish 5 — MSVC has a completely different
    // linker surface (`link.exe` / `lld-link.exe`: `/OUT:foo.exe`,
    // `/SUBSYSTEM:CONSOLE`, `.lib` system libs, no `-l` flag). Route
    // through a dedicated path; everything else (Mach-O, ELF, MinGW
    // PE-COFF) shares the gcc-style cc-driver path below.
    if target.is_msvc() {
        return link_aot_binary_msvc(
            bytecode_obj,
            cmain_obj,
            traces_obj,
            staticlib,
            out_path,
            target,
        );
    }

    let mut cmd = target.cc_command();

    // Object files first (they reference symbols defined in the
    // staticlib). Order matters for some traditional Unix linkers
    // (`ld` resolves left-to-right; modern `lld` is order-independent
    // but we keep the canonical order for portability).
    cmd.arg(cmain_obj).arg(bytecode_obj);
    if let Some(traces) = traces_obj {
        // Trace mcode `.o` from Stage 7 sub-piece 4. Placed after the
        // bytecode object (which references the AOT trace `luna_aot_
        // trace_*` symbols via its bracketed meta section's
        // relocations) so resolution flows correctly under traditional
        // left-to-right ld.
        cmd.arg(traces);
    }
    cmd.arg(staticlib);

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

/// v1.3 Phase AOT Stage 7 polish 5 — MSVC link path. Drives
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
fn link_aot_binary_msvc(
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

// ────────────────────────────────────────────────────────────────────
// v1.3 Phase AOT Stage 7 sub-piece 4 — offline trace harvester +
// AOT trace mcode emission.
//
// The pipeline:
//   1. Build a JIT-equipped warmup `Vm` (luna_jit::new_with_jit) and
//      swap in a `RecordingTraceCompiler` wrapper that forwards every
//      compile attempt to the real Cranelift backend AND captures the
//      input `TraceRecord` into a thread-local. The recorder runs at
//      every back-edge that crosses the const `TRACE_HOT_THRESHOLD = 64`,
//      so simple counted loops with iteration counts in the thousands
//      will close at least one trace.
//   2. Load the dump and call the chunk's root closure. The dispatcher
//      records + compiles + dispatches as normal; we only care about
//      the captured records.
//   3. For each captured (proto, record) pair, re-lower the record via
//      `lower_trace_into_named` against a fresh `ObjectModule` per .o,
//      writing the emitted bytes + a per-trace `luna_trace_blob` payload
//      + a `luna_trace_meta` 48-byte index entry. All three sections are
//      bracket-symbol enumerated by the deploy walker at startup.
//   4. Bail-tolerantly: if no traces close (small / non-loopy source),
//      return `HarvestedTraces::None` and the pipeline skips the trace .o.
// ────────────────────────────────────────────────────────────────────

/// Records every `TraceRecord` the dispatcher tries to compile; forwards
/// the actual compile to the wrapped real Cranelift backend so the
/// warmup run dispatches normally afterwards.
///
/// Records are appended to a thread-local Vec — read out after the
/// warmup `vm.call_value` returns.
struct RecordingTraceCompiler {
    inner: luna_jit::jit_backend::CraneliftBackend,
}

thread_local! {
    /// Thread-local capture buffer for `TraceRecord`s observed during
    /// the AOT warmup run. Each entry is `(head_proto_hash,
    /// head_pc, TraceRecord)`. Cleared at the start of every
    /// [`harvest_and_emit_aot_traces`] call.
    static AOT_CAPTURED_RECORDS: std::cell::RefCell<Vec<([u8; 16], u32, TraceRecord)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl luna_core::jit::TraceCompiler for RecordingTraceCompiler {
    fn try_compile_trace(
        &self,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        // Capture a cloneable image of the record so we can re-lower at
        // AOT emit time. TraceRecord is Clone since every field is
        // Clone (Gc<Proto> = NonNull copy; Vec<RecordedOp> is Clone).
        let hash = record.head_proto.stable_hash();
        AOT_CAPTURED_RECORDS.with(|cell| {
            cell.borrow_mut()
                .push((hash, record.head_pc, record.clone()));
        });
        self.inner.try_compile_trace(record, opts)
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        self.inner.last_compile_checkpoint()
    }
}

/// Run a warmup `Vm` on the dumped bytecode, harvest closed
/// `TraceRecord`s the trace JIT compiled, re-lower each through
/// `lower_trace_into_named` into a fresh `ObjectModule`, and write the
/// produced bytes (plus a `luna_trace_meta` index + `luna_trace_blob`
/// payload) to `out`.
///
/// Returns `Ok(HarvestedTraces::None)` if the warmup recorded zero
/// AOT-installable traces — the calling pipeline then skips the .o on
/// the link line entirely (no placeholder .o on disk).
///
/// `dump_bytes` is the same chunk the deploy binary will execute, so
/// the proto identities (and therefore `stable_hash`) match between
/// AOT compile and deploy load.
fn harvest_and_emit_aot_traces(
    dump_bytes: &[u8],
    version: LuaVersion,
    out: &Path,
    target: &TargetSpec,
) -> Result<HarvestedTraces, AotError> {
    use cranelift_codegen::settings::{self, Configurable};
    use cranelift_module::{DataDescription, Linkage, Module, default_libcall_names};
    use cranelift_object::{ObjectBuilder, ObjectModule};

    // Reset the capture buffer for this harvest call. A previous run
    // in the same process (e.g. unit tests running back-to-back) must
    // not leak its records into this one.
    AOT_CAPTURED_RECORDS.with(|cell| cell.borrow_mut().clear());

    // Build the warmup Vm + install the recording compiler. We use
    // `new_with_jit` (= new_minimal_with_jit + open_all_libs) so the
    // script can call `print`, `math.*`, etc. — typical hot loops in
    // realistic programs touch these libs.
    let mut vm = luna_jit::new_with_jit(version);
    vm.install_jit_backend(
        luna_jit::jit_backend::CraneliftBackend,
        RecordingTraceCompiler {
            inner: luna_jit::jit_backend::CraneliftBackend,
        },
    );
    vm.set_trace_jit_enabled(true);
    vm.set_bytecode_loading(true);

    // Load + call the root closure. Errors here are **non-fatal** —
    // the script may rely on host-side state we don't have (CLI args,
    // env vars), may `error()` intentionally as part of normal control
    // flow, or may diverge from any reproducible path. The warmup's
    // job is to surface hot traces, not to validate the script —
    // produce 0 AOT traces and move on if anything goes sideways.
    //
    // The deploy binary still runs the script through interp + JIT;
    // a missing AOT fast-path doesn't break correctness, only perf.
    let closure = match vm.load(dump_bytes, b"=embedded-aot-warmup") {
        Ok(c) => c,
        Err(_) => return Ok(HarvestedTraces::None),
    };
    let _ = vm.call_value(Value::Closure(closure), &[]);

    // Snapshot the captured records. Take ownership so the thread-
    // local Vec is empty going forward (matches the test-isolation
    // invariant established at the top of this fn).
    let captured: Vec<([u8; 16], u32, TraceRecord)> =
        AOT_CAPTURED_RECORDS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));

    let probe_on = std::env::var_os("LUNA_AOT_HARVEST_PROBE").is_some();
    if probe_on {
        eprintln!("luna-aot harvest: captured {} TraceRecords", captured.len());
    }
    if captured.is_empty() {
        return Ok(HarvestedTraces::None);
    }

    // Pair each captured record with the actual CompiledTrace that
    // landed in the proto's traces vec — we need the post-lower fields
    // (window_size, entry_tags, exit_tags, dispatchable,
    // global_tag_res_kind) for the meta blob. The record-by-record
    // lookup keys on (proto_ptr, head_pc); records that didn't survive
    // compile (lowerer bailed, returned None) drop here.
    //
    // Filter to AOT-installable shapes. Wire format v2 carries
    // `per_exit_tags` (typed-register side-exit guards — GetUpval-
    // heavy traces). Wire format v3 *scaffolds* `per_exit_inline`
    // (depth>0 inlined cmp side-exits) but the harvester still
    // bails on non-empty inline today: the trace mcode side bakes
    // the `Rc<[FrameMaterializeInfo]>` chain pointer as a raw
    // `iconst` immediate at lower time (see `luna-jit/src/jit_
    // backend/trace.rs` near `chain_ptr =
    // std::rc::Rc::as_ptr(&chain_rc)`). Under AOT that immediate
    // would be the warmup VM's heap address, invalid in the deploy
    // binary; the trace would crash on the inline side-exit path.
    // Unlocking requires a per-site relocatable chain-slot scheme
    // analogous to the strkey slot pattern — module docs on
    // `luna-core::jit::aot_meta` v3 lay out the three pieces.
    // Until that lands the filter stays in place.
    //
    // The wire format also doesn't ship sunk-alloc materialize
    // sites yet; `materialize_emit_count > 0` traces need that
    // path too — JIT-only.
    let mut installable: Vec<(
        usize,
        [u8; 16],
        u32,
        TraceRecord,
        std::rc::Rc<CompiledTrace>,
    )> = Vec::new();
    let mut filter_stats = (0usize, 0usize, 0usize, 0usize);
    for (i, (hash, head_pc, record)) in captured.into_iter().enumerate() {
        let proto = record.head_proto;
        let traces_ref = proto.traces.borrow();
        let Some(ct) = traces_ref.iter().find(|c| c.head_pc == head_pc).cloned() else {
            filter_stats.0 += 1;
            continue;
        };
        drop(traces_ref);
        if !ct.dispatchable {
            filter_stats.1 += 1;
            continue;
        }
        if !ct.per_exit_inline.is_empty() {
            // v1.3 Phase AOT Stage 7 polish 6 — depth>0 inlined cmp
            // side-exits are NOW supported. The lowerer's
            // `emit_chain_ptr_arg` routes the `FrameMaterializeInfo`
            // chain pointer through a relocatable data slot the
            // deploy-side `aot_inline_chain_resolver` populates at
            // startup; the v3 wire format's `per_exit_inline` tail
            // (cont_pc / head_resume_pc / packed exit_tags / packed
            // chain bytes) round-trips into a fresh
            // `Rc<[InlineSideExit]>` on the install side. Stat counter
            // retained for diagnostics: how many of the captured
            // traces went through the v3 inline path.
            filter_stats.2 += 1;
        }
        // per_exit_tags is NOW supported by wire format v2 — accept.
        // (Counter retained for diagnostics: how many of the captured
        // traces went through the v2 path.)
        if !ct.per_exit_tags.is_empty() {
            filter_stats.3 += 1;
        }
        installable.push((i, hash, head_pc, record, ct));
    }

    if probe_on {
        eprintln!(
            "luna-aot harvest: installable={}, filtered: no_ct={}, undispatchable={}, accepted_with_per_exit_inline={}, accepted_with_per_exit_tags={}",
            installable.len(),
            filter_stats.0,
            filter_stats.1,
            filter_stats.2,
            filter_stats.3,
        );
    }
    if installable.is_empty() {
        return Ok(HarvestedTraces::None);
    }

    // Build the ObjectModule for the trace .o. PIC required for ELF/
    // Mach-O linker relocations.
    //
    // v1.3 Phase AOT Stage 7 polish 4: `cranelift_isa_builder()` resolves
    // the per-target ISA (host = `cranelift_native` for CPU-feature
    // detection; cross = `isa::lookup` over the parsed triple). When
    // target == host the ISA, flags, and resulting mcode are
    // byte-for-byte identical to the pre-polish path; for cross targets
    // we get a `TargetIsa` whose codegen matches the deploy ABI rather
    // than the build host's.
    let isa = {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("use_colocated_libcalls", "false")
            .expect("flag");
        flag_builder.set("is_pic", "true").expect("flag");
        flag_builder.set("opt_level", "speed").expect("flag");
        target
            .cranelift_isa_builder()?
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| {
                AotError::Object(format!(
                    "Cranelift ISA finish for target {}: {e}",
                    target.triple
                ))
            })?
    };
    let object_builder = ObjectBuilder::new(isa, "luna_aot_traces", default_libcall_names())
        .map_err(|e| AotError::Object(format!("ObjectBuilder: {e}")))?;
    let mut module = ObjectModule::new(object_builder);

    // Per-trace emission: lower IR + meta blob + index entry.
    // We accumulate the meta blobs into one combined `luna_trace_blob`
    // data symbol (so the linker has a single object per pipeline,
    // not N), and emit one `luna_trace_meta_<idx>` entry per trace.
    let mut blob_payload: Vec<u8> = Vec::new();
    // Per-trace `(fn_name, meta_blob_offset_within_combined, meta_blob_len)`.
    let mut per_trace_meta: Vec<(String, u32, u32)> = Vec::new();
    for (idx, _hash, _head_pc, record, ct) in installable.iter() {
        let fn_name = format!("luna_aot_trace_{idx:08x}");
        let opts = CompileOptions {
            internal_loop: true,
            pre53: matches!(version, LuaVersion::Lua51 | LuaVersion::Lua52),
            aot: true,
        };
        // Re-lower this record into the ObjectModule under a unique
        // exported name. Any bail here = the record was lowerable at
        // warmup time (it appeared in the captured list AND the
        // matched ct is non-None) but failed under aot=true codegen —
        // most likely a relocation path that fails for some opcode the
        // AOT lowerer's strkey resolver doesn't cover. Skip + continue;
        // the trace will fall back to JIT at deploy time.
        let lower_res = luna_jit::jit_backend::trace::lower_trace_into_named(
            &mut module,
            record,
            opts,
            Some(&fn_name),
        );
        if lower_res.is_none() {
            if probe_on {
                eprintln!(
                    "luna-aot harvest: AOT lower bailed for trace idx={idx} head_pc={}",
                    ct.head_pc
                );
            }
            continue;
        }

        // Serialize the meta blob for this trace.
        let entry_tags_vec: Vec<u8> = ct.entry_tags.iter().copied().collect();
        let exit_tags_vec: Vec<u8> = ct.exit_tags.iter().copied().map(pack_exit_tag).collect();
        // v2 tail — per-cont_pc typed-register side-exit shapes.
        // Each entry's `tags_packed` mirrors the JIT-time
        // `(cont_pc, Rc<[ExitTag]>)` pair, packed through
        // `pack_exit_tag` so the deploy-side reader unpacks via the
        // same byte → ExitTag mapping.
        let per_exit_tags_entries: Vec<PerExitTagsEntry> = ct
            .per_exit_tags
            .iter()
            .map(|(cont_pc, tags)| PerExitTagsEntry {
                cont_pc: *cont_pc,
                tags_packed: tags.iter().copied().map(pack_exit_tag).collect(),
            })
            .collect();
        let header = AotTraceMetaHeader {
            magic: luna_core::jit::aot_meta::AOT_META_MAGIC,
            version: luna_core::jit::aot_meta::AOT_META_VERSION,
            head_pc: ct.head_pc,
            n_ops: ct.n_ops,
            window_size: ct.window_size,
            dispatchable: u8::from(ct.dispatchable),
            tag_res_kind: pack_tag_res_kind(ct.global_tag_res_kind),
            entry_tags_len: entry_tags_vec.len() as u16,
            exit_tags_len: exit_tags_vec.len() as u32,
        };
        // v1.3 Phase AOT Stage 7 polish 6 — populate the v3 inline
        // tail from the live trace's `per_exit_inline`. Each
        // `InlineSideExit` round-trips into a `PerExitInlineEntry`
        // via the byte-stable converter: tags pack through
        // `pack_exit_tag` and the chain serialises as raw `repr(C)`
        // bytes (12 per record). The deploy install path decodes
        // the entries back into fresh `Rc<[FrameMaterializeInfo]>` /
        // `Rc<[ExitTag]>` allocations whose contents match what the
        // JIT-time recorder produced; the IR's chain pointer is fed
        // from a separate `aot_inline_chain_resolver`-populated
        // slot (no shared ownership with the dispatcher field —
        // neither side compares pointers).
        let per_exit_inline_entries: Vec<PerExitInlineEntry> = ct
            .per_exit_inline
            .iter()
            .map(PerExitInlineEntry::from_inline_side_exit)
            .collect();
        let blob = encode_meta_blob(
            &header,
            &entry_tags_vec,
            &exit_tags_vec,
            &per_exit_tags_entries,
            &per_exit_inline_entries,
        );
        let blob_offset = blob_payload.len() as u32;
        let blob_len = blob.len() as u32;
        blob_payload.extend_from_slice(&blob);
        per_trace_meta.push((fn_name, blob_offset, blob_len));
    }

    if per_trace_meta.is_empty() {
        // Everything filtered out by the AOT lower bail. Same shape as
        // "no traces at all".
        return Ok(HarvestedTraces::None);
    }

    // Emit the combined `luna_trace_blob` data symbol. Single object;
    // each per-trace meta entry references it via offset.
    let blob_data_id = module
        .declare_data("__luna_trace_blob_combined", Linkage::Local, false, false)
        .map_err(|e| AotError::Object(format!("declare_data blob: {e}")))?;
    {
        let mut desc = DataDescription::new();
        desc.define(blob_payload.into_boxed_slice());
        // Read-only data section. Mach-O caps section names at 16 chars;
        // `luna_trace_blob` is 15 — fits with one byte of headroom for
        // the implicit comparator NUL.
        // `__DATA` segment on Mach-O — the deploy walker doesn't
        // bracket this section (each meta entry references it by
        // pointer relocation), but keeping it next to `luna_trace_
        // meta` in `__DATA` is the path of least surprise for ld /
        // strip.
        //
        // Windows COFF host (Stage 7 polish 3): PE section names are
        // capped at 8 bytes in the final image; use `.lt_blob` (7 chars
        // + leading `.`) so the post-link PE preserves the name
        // byte-for-byte. The deploy walker doesn't bracket-look this
        // section (it's only referenced via pointer relocations from
        // `.lt_meta` entries), so the name choice is mostly for
        // consistency / debuggability.
        let (blob_seg, blob_sect) = if cfg!(target_os = "windows") {
            ("", ".lt_blob")
        } else {
            ("__DATA", "luna_trace_blob")
        };
        desc.set_segment_section(blob_seg, blob_sect);
        module
            .define_data(blob_data_id, &desc)
            .map_err(|e| AotError::Object(format!("define_data blob: {e}")))?;
    }

    // Emit one 48-byte `luna_trace_meta_<idx>` index entry per trace
    // into the dedicated `luna_trace_meta` section. The static linker
    // auto-brackets via `__start_luna_trace_meta` / `__stop_luna_trace_
    // meta` (ELF) or `section$start$__DATA$luna_trace_meta` (Mach-O).
    for (idx, (fn_name, blob_offset, blob_len)) in per_trace_meta.iter().enumerate() {
        let hash = installable[idx].1;
        let head_pc = installable[idx].2;

        let entry_data_id = module
            .declare_data(
                &format!("__luna_trace_meta_entry_{idx:08x}"),
                Linkage::Local,
                false,
                false,
            )
            .map_err(|e| AotError::Object(format!("declare_data meta entry: {e}")))?;
        let mut desc = DataDescription::new();
        // 48 bytes: [hash 16] [head_pc 4] [_pad 4] [fn_ptr 8] [meta_ptr 8] [meta_len 4] [_pad2 4]
        let mut payload = [0u8; 48];
        payload[0..16].copy_from_slice(&hash);
        payload[16..20].copy_from_slice(&head_pc.to_le_bytes());
        // payload[20..24] _pad
        // payload[24..32] fn_ptr — relocation
        // payload[32..40] meta_ptr — relocation
        payload[40..44].copy_from_slice(&blob_len.to_le_bytes());
        desc.define(Box::new(payload));
        // Use `__DATA` segment explicitly on Mach-O so the section
        // merges with the cmain shim's `__DATA,luna_trace_meta`
        // placeholder. cranelift_object passes the segment arg
        // through verbatim; an empty segment lands the section in
        // segment `""`, separate from `__DATA` — and the deploy
        // walker's `section$start$__DATA$luna_trace_meta` would only
        // see the placeholder. ELF / PE ignore the segment arg
        // (segment concept is Mach-O specific) so passing `__DATA`
        // is a no-op there.
        //
        // Windows COFF host (Stage 7 polish 3): short name `.lt_meta`
        // matches the cmain shim's placeholder section, and the
        // deploy walker's [`windows_section::find_section`] needle.
        // PE section names are capped at 8 bytes in the final linked
        // image — `luna_trace_meta` (15 chars) would either truncate
        // unpredictably or land in the COFF string table that the
        // linker drops.
        let (meta_seg, meta_sect) = if cfg!(target_os = "windows") {
            ("", ".lt_meta")
        } else {
            ("__DATA", "luna_trace_meta")
        };
        desc.set_segment_section(meta_seg, meta_sect);
        // 8-byte alignment for the fn_ptr / meta_ptr relocations at
        // offsets 24 and 32. Without explicit `set_align(8)` the
        // entries can land at odd offsets in the .o, and Mach-O's
        // `ld` rejects the unaligned pointer slots ("pointer not
        // aligned in `___luna_trace_meta_entry_…`+0x20").
        desc.set_align(8);

        // Declare the trace fn as Import so we can relocate against it.
        // (It's Export'd by `lower_trace_into_named` above.) The
        // declare_function call here re-uses the same name; cranelift
        // module name-interning matches the two so the relocation
        // resolves to the lowerer-emitted body.
        let trace_fn_sig = {
            let mut sig = module.make_signature();
            sig.params.push(cranelift_codegen::ir::AbiParam::new(
                cranelift_codegen::ir::types::I64,
            ));
            sig.returns.push(cranelift_codegen::ir::AbiParam::new(
                cranelift_codegen::ir::types::I64,
            ));
            sig
        };
        let fn_id = module
            .declare_function(fn_name, Linkage::Import, &trace_fn_sig)
            .map_err(|e| AotError::Object(format!("declare_function for reloc: {e}")))?;
        let fn_gv = module.declare_func_in_data(fn_id, &mut desc);
        desc.write_function_addr(24, fn_gv);
        let blob_gv = module.declare_data_in_data(blob_data_id, &mut desc);
        desc.write_data_addr(32, blob_gv, *blob_offset as i64);

        module
            .define_data(entry_data_id, &desc)
            .map_err(|e| AotError::Object(format!("define_data meta entry: {e}")))?;
    }

    let product = module.finish();
    let bytes = product
        .emit()
        .map_err(|e| AotError::Object(format!("ObjectProduct::emit: {e}")))?;
    fs::write(out, &bytes)?;
    Ok(HarvestedTraces::Some)
}
