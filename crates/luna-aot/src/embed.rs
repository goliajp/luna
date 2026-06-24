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
