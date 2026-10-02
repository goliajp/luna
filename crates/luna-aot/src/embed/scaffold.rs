//! The scaffold path: a host-only bytecode object plus a C entry that
//! prints the embedded section size.

use std::fs;
use std::path::Path;
use std::process::Command;

use object::write::{Object, Symbol, SymbolSection};
use object::{SectionKind, SymbolKind, SymbolScope};

use super::AotError;
use super::target::host_object_target;
use crate::{BYTECODE_END_SYMBOL, BYTECODE_SECTION_NAME, BYTECODE_START_SYMBOL};

/// Emit `.o` containing a single `.luna.bytecode` data section with
/// the dump bytes, bracketed by two **global** symbols
/// `__luna_bytecode_start` / `__luna_bytecode_end`. Mach-O symbols
/// are prefixed with `_` so the C linker resolves the bare names.
pub(super) fn write_bytecode_object(bytecode: &[u8], out: &Path) -> Result<(), AotError> {
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
    // per `Mangling::global_prefix`.
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
/// This is the **scaffold runtime**. [`compile_and_link`] links the
/// real runtime instead, which constructs a `Vm`, loads the embedded
/// bytecode, and runs it.
pub(super) fn write_scaffold_entry_object(out: &Path) -> Result<(), AotError> {
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
    // best-effort: leave the .c around for diagnosis
    Ok(())
}

/// Invoke `cc` (or `$CC`) to link object files into `out_path`.
pub(super) fn link_with_cc(objects: &[&Path], out_path: &Path) -> Result<(), AotError> {
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
