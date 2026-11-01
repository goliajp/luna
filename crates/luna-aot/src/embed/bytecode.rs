//! A binary that only carries a script's bytecode, without the runtime.

use super::*;

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
/// The produced binary is **runnable**: it prints the embedded bytecode length to `stderr` and
/// exits 0. It does not execute the bytecode; use [`compile_and_link`] for a binary that runs the
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
    // SAFETY: the heap outlives every handle made from it here, and its objects stay in it
    let mut heap = unsafe { luna_core::__private::new_heap() };
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
