//! The front end of an AOT build: a Lua source to the dump it embeds.

use std::fs;
use std::path::Path;

use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::runtime::Heap;
use luna_core::version::LuaVersion;
use luna_core::vm::dump;

use super::AotError;

/// Parse + compile and produce the dump bytes the
/// bytecode object holds. Factored out so [`embed_bytecode`] and
/// [`compile_and_link`] share the front-end exactly.
pub(super) fn compile_to_dump(
    source_path: &Path,
    version: LuaVersion,
) -> Result<Vec<u8>, AotError> {
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
