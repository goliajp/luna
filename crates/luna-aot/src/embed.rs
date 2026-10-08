//! The AOT pipeline:
//! parse + compile Lua source → dump luna bytecode → embed into an
//! object file's `.luna.bytecode` data section → link.
//!
//! [`embed_bytecode`] is the **scaffold** path: it ends after the
//! bytecode is in a `.luna.bytecode` section bracketed by the two
//! public symbols [`crate::BYTECODE_START_SYMBOL`] and
//! [`crate::BYTECODE_END_SYMBOL`], linked against a C entry that only
//! prints the section size. [`compile_and_link`] is the full path: it
//! also links the `luna-runtime-helpers` staticlib and any AOT trace
//! mcode harvested from a warmup run.
//!
//! # Scaffold pipeline
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
//!   Vec<u8>   ── luna body, "\x1bLua" + dialect header + "\x00LunaV4\x00" sentinel + body
//!     │  object::write::Object  (this module)
//!     ▼
//!   foo.luna_bytecode.o   (ELF / Mach-O / PE — host triple)
//!     │  cc foo.luna_bytecode.o entry_stub.o -o foo
//!     ▼
//!   foo   (native binary; scaffold entry prints the section size)
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::runtime::Heap;
use luna_core::version::LuaVersion;
use luna_core::vm::dump;

mod error;
mod harvest;
mod link;
mod msvc_link;
mod msvc_sysroot;
mod scaffold;
mod staticlib;
mod target;
mod trace_object;

pub use error::AotError;
use harvest::harvest_and_emit_aot_traces;
use link::{link_aot_binary_for, write_aot_cmain_object_for, write_bytecode_object_for};
use scaffold::{link_with_cc, write_bytecode_object, write_scaffold_entry_object};
use staticlib::build_runtime_helpers_staticlib;
use target::host_triple;
pub use target::{TargetLibc, TargetOs, TargetSpec};
/// Compile `source_path` into `out_path` (a native binary embedding
/// the dumped luna bytecode in a `.luna.bytecode` section).
///
/// `target_triple` is parsed only for the host-vs-cross check; the
/// scaffold only supports the host triple. Pass `None` to default to
/// the host.
///
/// `version` selects the Lua dialect for parsing + bytecode emit
/// (defaults to [`LuaVersion::Lua55`] when called via the CLI).
///
/// # End-to-end behaviour
///
/// The produced binary is **runnable**: it prints the embedded
/// bytecode length to `stderr` and exits 0. It does not execute the
/// bytecode; use [`compile_and_link`] for a binary that runs the
/// script through a `Vm`.
pub fn embed_bytecode(
    source_path: &Path,
    out_path: &Path,
    target_triple: Option<&str>,
    version: LuaVersion,
) -> Result<(), AotError> {
    if let Some(t) = target_triple
        && t != host_triple()
    {
        return Err(AotError::UnsupportedTarget(t.to_string()));
    }

    // source → AST → Proto. Uses the same path the runtime
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

    // Write the bytecode object file.
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

    // Link via system `cc`. The scaffold uses a minimal C entry that
    // references the bracket symbols (proves the section is reachable
    // end-to-end).
    link_with_cc(&[&bytecode_obj_path, &stub_obj_path], out_path)?;

    Ok(())
}

// ────────────────────────────────────────────────────────────────────
// Interp-runtime link path
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
//      + extern-decls `luna_aot_run_dialect`, then calls
//      `luna_aot_run_dialect(start, end - start, dialect)`. Compile via `cc -c`.
//   5. Link bytecode.o + main.o + libluna_runtime_helpers.a +
//      platform libs (`-lpthread -ldl -lm -framework CoreFoundation`
//      on Mac) into the final binary.
//
// Cranelift trace mcode emission is a separate concern — it adds a
// third object file (containing the lowered traces) to the
// link line. The interp-runtime fallback path lives in the staticlib
// either way, so adding the trace.o is purely additive.
// ────────────────────────────────────────────────────────────────────

/// End-to-end AOT compile: produces a self-contained binary that, when
/// run, loads the embedded bytecode through a luna `Vm` and executes
/// it. Traces harvested from a warmup run are linked in as AOT mcode.
///
/// Differs from [`embed_bytecode`]:
/// - Builds and links `luna-runtime-helpers` (staticlib carrying
///   luna-core + a `luna_aot_run_dialect` C-ABI entry, which runs the
///   chunk on a `Vm` of `version`).
/// - Produced binary actually **runs** the script — `print(...)` lands
///   on stdout, runtime errors print to stderr + exit 1, etc.
///
/// `target_triple` selects a cross target via [`TargetSpec::from_triple`];
/// cross-compile builds a per-triple staticlib (`cargo build
/// --target=<triple> -p luna-runtime-helpers`) and uses the matching
/// cc driver.
///
/// For a Windows target, an `out_path` without an extension is written
/// as `<out_path>.exe`. An MSVC target needs `cl.exe` + `link.exe`
/// (found in the Visual Studio install on a Windows host, no Developer
/// Command Prompt needed) or LLVM's `clang-cl` + `lld-link` on `PATH`.
pub fn compile_and_link(
    source_path: &Path,
    out_path: &Path,
    target_triple: Option<&str>,
    version: LuaVersion,
) -> Result<(), AotError> {
    compile_and_link_with(
        source_path,
        out_path,
        target_triple,
        version,
        AotOptions::from_env(),
    )
}

/// Settings for [`compile_and_link_with`].
#[derive(Clone, Copy, Debug, Default)]
pub struct AotOptions {
    /// Print why each harvested trace was or was not linked in.
    pub harvest_probe: bool,
}

impl AotOptions {
    /// The settings [`compile_and_link`] uses: `harvest_probe` is on when
    /// `LUNA_AOT_HARVEST_PROBE` is set.
    pub fn from_env() -> Self {
        Self {
            harvest_probe: std::env::var_os("LUNA_AOT_HARVEST_PROBE").is_some(),
        }
    }
}

/// [`compile_and_link`] with explicit [`AotOptions`] instead of reading
/// them from the environment.
pub fn compile_and_link_with(
    source_path: &Path,
    out_path: &Path,
    target_triple: Option<&str>,
    version: LuaVersion,
    options: AotOptions,
) -> Result<(), AotError> {
    // Resolve the target: explicit triple if supplied, else host. Anything
    // we can't describe (object-file format, cc invocation, lib set)
    // surfaces as `UnsupportedTarget` here — no silent fallback to host.
    let target = match target_triple {
        Some(t) => TargetSpec::from_triple(t)?,
        None => TargetSpec::host(),
    };

    // Parse + compile + dump: shared with `embed_bytecode`.
    let dump_bytes = compile_to_dump(source_path, version)?;

    // MinGW's gcc names its output `<out>.exe` when `<out>` has no
    // extension and link.exe does not; give every Windows target the
    // suffix so the binary lands at one predictable path
    let exe_path;
    let out_path = if target.os == TargetOs::Windows && out_path.extension().is_none() {
        exe_path = out_path.with_extension("exe");
        exe_path.as_path()
    } else {
        out_path
    };

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

    // Bytecode object — target-aware format/arch.
    write_bytecode_object_for(&dump_bytes, &bytecode_obj_path, &target)?;

    // Tiny C main that calls into the staticlib. The C source
    // is target-independent (extern decls only); the `cc -c` invocation
    // routes through the target-aware cc driver so the .o has the right
    // ABI / object-format magic.
    write_aot_cmain_object_for(&cmain_obj_path, &target, version)?;

    // Offline trace recorder + AOT trace mcode emission. The warmup `Vm` always runs on the **host**
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
        match harvest_and_emit_aot_traces(
            &dump_bytes,
            version,
            &path,
            &target,
            options.harvest_probe,
        )? {
            HarvestedTraces::None => None,
            HarvestedTraces::Some => Some(path),
        }
    };

    // Ensure the runtime staticlib exists for `target`.
    // For the host triple this is a workspace cargo build; for a cross
    // triple it's `cargo build --target=<triple>` and the resulting
    // staticlib lives under `target/<triple>/release-aot-helpers/`.
    // The `release-aot-helpers` profile (workspace `Cargo.toml`) has
    // `lto = "off"` so the 39 `luna_jit_*` Cranelift trace-mcode
    // helpers survive the rlib → staticlib bundling step.
    let staticlib =
        build_runtime_helpers_staticlib(target.triple_for_cargo(), &target.staticlib_build_env()?)?;

    // Final link via the target's cc driver. Order matters on
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

/// Return shape for [`harvest_and_emit_aot_traces`]. `None` = warmup recorded zero
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

/// Parse + compile and produce the dump bytes the
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
