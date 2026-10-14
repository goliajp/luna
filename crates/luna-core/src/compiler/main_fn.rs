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
    let source = heap.try_intern(source_name)?;
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
        depth: 0,
        pool: scratch.levels.take(),
        sym_strs: scratch.sym_strs.take(),
        last_line: 0,
        force_line: None,
        str_cache: LMap::new(mem),
        spine: scratch.spine.take(),
        lvs: scratch.lvs.take(),
        operands: scratch.operands.take(),
        cur: TokRef::Start,
        early_emit: false,
        next_pending: false,
        err_tok: None,
        puc_log: {
            let mut log = scratch.puc_log.take();
            log.clear();
            log
        },
        defer_locvars: false,
        pending_locvars: 0,
    };
    c.sym_strs.clear();
    c.sym_strs.resize_or_abort(ast.names.len(), None);
    c.open_level(0, true, 0);
    c.l().upvals.push_or_abort(UpvalDesc {
        in_stack: false,
        index: 0,
        name: "_ENV".into(),
        read_only: false,
    });
    // `setvararg`, then the `_ENV` upvalue (5.1 has none)
    c.log(super::puc_log::PEv::Prologue);
    if version != LuaVersion::Lua51 {
        c.log(super::puc_log::PEv::Upval(0));
    }
    c.enter_block(false);
    scratch.err_tok = None;
    let body = c.stat_block(&ast.block).and_then(|()| {
        // the implicit final return belongs to the chunk's last line (PUC),
        // so a line hook / activelines see it there rather than on the last
        // statement
        c.at(TokRef::Eof);
        c.final_return(ast.end_line)
    });
    if let Err(e) = body {
        scratch.err_tok = c.err_tok.take();
        scratch.puc_log = std::mem::replace(&mut c.puc_log, LVec::new(c.heap.mem()));
        return Err(e);
    }
    let last_target = c.lr().last_target;
    let proto = c.finish_level(0, 0);
    for lvl in c.levels.drain_all() {
        c.pool.push_or_abort(lvl.into_bufs());
    }
    scratch.levels = c.pool;
    scratch.sym_strs = c.sym_strs;
    c.spine.clear();
    scratch.spine = c.spine;
    scratch.lvs = c.lvs;
    scratch.operands = c.operands;
    scratch.puc_log = c.puc_log;
    scratch.open = c.levels.recycle();
    Ok((proto, last_target))
}
