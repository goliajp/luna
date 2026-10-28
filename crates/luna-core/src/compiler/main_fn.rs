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
    compile_main((ast, &[], None), version, source, heap, &mut scratch)
}

/// Lower an [`ast::Chunk`] into a [`Proto`] (luna bytecode) for the
/// given dialect. The interned source name is attached to the proto for
/// error messages and `debug.getinfo`.
pub fn compile_chunk(
    ast: &ast::Chunk,
    version: LuaVersion,
    source_name: &[u8],
    heap: &mut Heap,
) -> Result<Gc<Proto>, SyntaxError> {
    let mut scratch = CompileScratch::new(heap.mem());
    let source = heap.try_intern(source_name)?;
    compile_parsed(ast, &[], None, version, source, heap, &mut scratch)
}

/// [`compile_chunk`] with the `end` lines the parser recorded for loops
/// ([`crate::frontend::parser::Parsed::end_lines`]); a [`ast::Chunk`] carries no
/// such lines, so code PUC emits after a loop's `end` is placed on that
/// line only when they are given. The functions are built in the vectors
/// of `scratch`; what PUC allocates is logged when `marks`, the parse's
/// recording, are given.
pub(crate) fn compile_parsed(
    ast: &Chunk,
    end_lines: &[u32],
    marks: Option<&crate::frontend::parser::Marks>,
    version: LuaVersion,
    source: Gc<LuaStr>,
    heap: &mut Heap,
    scratch: &mut CompileScratch,
) -> Result<Gc<Proto>, SyntaxError> {
    let parsed = (ast, end_lines, marks);
    compile_main(parsed, version, source, heap, scratch).map(|(p, _)| p)
}

/// Compile the main function; also gives its `last_target`.
pub(super) fn compile_main(
    parsed: Parsed<'_>,
    version: LuaVersion,
    source: Gc<LuaStr>,
    heap: &mut Heap,
    scratch: &mut CompileScratch,
) -> Result<(Gc<Proto>, Option<usize>), SyntaxError> {
    crate::cerrno::begin_folds();
    let r = compile_main_body(parsed, version, source, heap, scratch);
    crate::cerrno::end_folds();
    r
}

/// A parsed chunk: its tree, its loops' `end` lines, and the marks the
/// parse recorded, if it did.
type Parsed<'a> = (
    &'a Chunk,
    &'a [u32],
    Option<&'a crate::frontend::parser::Marks>,
);

fn compile_main_body(
    parsed: Parsed<'_>,
    version: LuaVersion,
    source: Gc<LuaStr>,
    heap: &mut Heap,
    scratch: &mut CompileScratch,
) -> Result<(Gc<Proto>, Option<usize>), SyntaxError> {
    let ast = parsed.0;
    let mut c = compiler_from(parsed, version, source, heap, scratch);
    c.strs.reset(ast.names.len());
    c.open_level(0, true, 0);
    c.push_upval(
        UpvalDesc {
            in_stack: false,
            index: 0,
            name: "_ENV".into(),
            read_only: false,
        },
        load_strs::ENV,
    );
    // `setvararg`, then the `_ENV` upvalue (5.1 has none)
    c.log(super::puc_log::PEv::Prologue);
    if version != LuaVersion::Lua51 {
        c.log(super::puc_log::PEv::Upval(0));
    }
    c.enter_block(false);
    scratch.err_tok = None;
    (scratch.replay.sealed, scratch.replay.at) = (None, None);
    let body = c.stat_block(&ast.block).and_then(|()| {
        // the implicit final return belongs to the chunk's last line (PUC),
        // so a line hook / activelines see it there rather than on the last
        // statement
        c.at(TokRef::Eof);
        c.final_return(ast.end_line)
    });
    if let Err(e) = body {
        scratch.err_tok = c.err_tok.take();
        if scratch.replay.seal {
            scratch.replay.sealed = Some(c.seal_levels());
        }
        c.settle_log();
        scratch.puc_log = std::mem::replace(&mut c.puc_log, LVec::new(c.heap.mem()));
        return Err(e);
    }
    let last_target = c.lr().last_target;
    let proto = c.finish_level(0, 0);
    c.settle_log();
    keep_scratch(c, scratch);
    Ok((proto, last_target))
}

/// A compiler over `ast`, its vectors taken from `scratch`.
fn compiler_from<'a>(
    (ast, end_lines, marks): Parsed<'a>,
    version: LuaVersion,
    source: Gc<LuaStr>,
    heap: &'a mut Heap,
    scratch: &mut CompileScratch,
) -> Compiler<'a> {
    let mem = heap.mem();
    Compiler {
        ast,
        end_lines,
        heap,
        version,
        source,
        levels: scratch.open.take().recycle(),
        depth: 0,
        pool: scratch.levels.take(),
        strs: std::mem::replace(&mut scratch.strs, LoadStrs::new(mem)),
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
        marks,
        puc_state: super::puc_log::LogState {
            lenient: scratch.replay.lenient,
            ..Default::default()
        },
    }
}

/// The vectors of `c`, which compiled a chunk, kept in `scratch` for the
/// next load.
fn keep_scratch(mut c: Compiler<'_>, scratch: &mut CompileScratch) {
    for lvl in c.levels.drain_all() {
        c.pool.push_or_abort(lvl.into_bufs());
    }
    scratch.levels = c.pool;
    scratch.strs = c.strs;
    c.spine.clear();
    scratch.spine = c.spine;
    scratch.lvs = c.lvs;
    scratch.operands = c.operands;
    scratch.puc_log = c.puc_log;
    scratch.open = c.levels.recycle();
}
