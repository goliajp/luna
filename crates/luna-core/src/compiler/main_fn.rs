//! Compiling a chunk's main function.

use super::*;

/// Diagnostic version of [`compile_chunk`] that also returns the main
/// proto's final `last_target` value (the highest pc recorded as a jump
/// destination — PUC `fs->lasttarget` equivalent). Used by the
/// jump-target tracker unit tests at
/// `crates/luna-core/tests/it/compiler_jump_target_tracker.rs`.
pub fn compile_chunk_with_last_target(
    ast: &ast::Chunk,
    version: LuaVersion,
    source_name: &[u8],
    heap: &mut Heap,
) -> Result<(Gc<Proto>, Option<usize>), SyntaxError> {
    let mut scratch = CompileScratch::new(heap.mem());
    let source = heap.intern(source_name);
    compile_main(ast, &[], version, source, heap, &mut scratch)
}

/// Compile the main function; also gives its `last_target`.
pub(super) fn compile_main(
    ast: &Chunk,
    end_lines: &[u32],
    version: LuaVersion,
    source: Gc<LuaStr>,
    heap: &mut Heap,
    scratch: &mut CompileScratch,
) -> Result<(Gc<Proto>, Option<usize>), SyntaxError> {
    crate::cerrno::begin_folds();
    let r = compile_main_body(ast, end_lines, version, source, heap, scratch);
    crate::cerrno::end_folds();
    r
}

fn compile_main_body(
    ast: &Chunk,
    end_lines: &[u32],
    version: LuaVersion,
    source: Gc<LuaStr>,
    heap: &mut Heap,
    scratch: &mut CompileScratch,
) -> Result<(Gc<Proto>, Option<usize>), SyntaxError> {
    let mem = heap.mem();
    let mut c = Compiler {
        ast,
        end_lines,
        heap,
        version,
        source,
        levels: scratch.open.take().recycle(),
        pool: scratch.levels.take(),
        sym_strs: scratch.sym_strs.take(),
        last_line: 0,
        force_line: None,
        str_cache: LMap::new(mem),
    };
    c.sym_strs.clear();
    c.sym_strs.resize_or_abort(ast.names.len(), None);
    let mut main = c.new_level(0, true, 0);
    main.upvals.push_or_abort(UpvalDesc {
        in_stack: false,
        index: 0,
        name: "_ENV".into(),
        read_only: false,
    });
    c.levels.push_or_abort(main);
    c.enter_block(false);
    c.stat_block(&ast.block)?;
    // the implicit final return belongs to the chunk's last line (PUC), so a
    // line hook / activelines see it there rather than on the last statement
    c.final_return(ast.end_line)?;
    let lvl = c.levels.pop().expect("main level");
    let last_target = lvl.last_target;
    let proto = c.finish_level(lvl, 0, 0);
    scratch.levels = c.pool;
    scratch.sym_strs = c.sym_strs;
    scratch.open = c.levels.recycle();
    Ok((proto, last_target))
}
