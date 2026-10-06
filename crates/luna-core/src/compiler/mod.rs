//! AST → bytecode compiler. Register model follows PUC lparser/lcode:
//! locals pin the low registers, temporaries grow from `freereg`, constants
//! are deduplicated, forward jumps are patch lists (plain Vecs instead of
//! PUC's in-code jump chains). Function nesting is a stack of `Level`s;
//! upvalue resolution walks it (PUC singlevaraux).
//!
//! Slice 3 state: calls, closures, upvalues, varargs (5.5 table semantics),
//! generic `for`, multret, tail calls. Still pending (slice 5): goto/labels,
//! `<close>`, `global` declarations.

use std::collections::HashMap;

mod assign;
mod assign_gate;
mod binop;
mod binop_const;
mod closure;
mod cond;
mod const_map;
mod control;
mod ctconst;
mod emit;
mod expr;
mod expr_names;
mod expr_ops;
mod fold;
mod level;
mod limits;
mod resolve;
mod retarget;
mod return_stat;
mod scope;
mod small_list;
mod stat;
mod table_ctor;
mod vararg_scan;
use const_map::{ConstKey, ConstMap};
use ctconst::{CtConst, ct_operand, ct_value};
use fold::{fold_arith, is_logical, numeral};
pub(crate) use level::CompileScratch;
use level::{Level, LevelBufs};
use limits::{MAX_LOCALS, max_regs, max_upvals};
use retarget::is_retargetable_op;
use small_list::{Jumps, SmallList};

use crate::frontend::ast::{
    self, AttribName, BinOp, Block, Chunk, Expr, ExprId, FuncBody, FuncName, List, ListItem, Name,
    Stat, StatId, TableField, UnOp, block_uses_vararg,
};
use crate::frontend::error::SyntaxError;
use crate::numeric::Num;
use crate::runtime::heap::{GcHeader, ObjTag};
use crate::runtime::{Gc, Heap, LuaStr, Proto, UpvalDesc, Value};
use crate::version::LuaVersion;
use crate::vm::isa::{Inst, MAX_B, MAX_BX, MAX_C, MAX_SC, MAX_SJ, MIN_SC, OFFSET_SC, Op};

/// Lower an [`ast::Chunk`] into a [`Proto`] (luna bytecode) for the
/// given dialect. The interned source name is attached to the proto for
/// error messages and `debug.getinfo`.
pub fn compile_chunk(
    ast: &ast::Chunk,
    version: LuaVersion,
    source_name: &[u8],
    heap: &mut Heap,
) -> Result<Gc<Proto>, SyntaxError> {
    let mut scratch = CompileScratch::default();
    let source = heap.intern(source_name);
    compile_parsed(ast, &[], version, source, heap, &mut scratch)
}

/// [`compile_chunk`] with the `end` lines the parser recorded for loops
/// ([`crate::frontend::parser::Parsed::end_lines`]); a [`ast::Chunk`] carries no
/// such lines, so code PUC emits after a loop's `end` is placed on that
/// line only when they are given. The functions are built in the vectors
/// of `scratch`.
pub(crate) fn compile_parsed(
    ast: &Chunk,
    end_lines: &[u32],
    version: LuaVersion,
    source: Gc<LuaStr>,
    heap: &mut Heap,
    scratch: &mut CompileScratch,
) -> Result<Gc<Proto>, SyntaxError> {
    compile_main(ast, end_lines, version, source, heap, scratch).map(|(p, _)| p)
}

/// Compile the main function; also gives its `last_target`.
fn compile_main(
    ast: &Chunk,
    end_lines: &[u32],
    version: LuaVersion,
    source: Gc<LuaStr>,
    heap: &mut Heap,
    scratch: &mut CompileScratch,
) -> Result<(Gc<Proto>, Option<usize>), SyntaxError> {
    let mut c = Compiler {
        ast,
        end_lines,
        heap,
        version,
        source,
        levels: level::relabel(std::mem::take(&mut scratch.open)),
        pool: std::mem::take(&mut scratch.levels),
        sym_strs: std::mem::take(&mut scratch.sym_strs),
        last_line: 0,
        force_line: None,
        str_cache: HashMap::new(),
    };
    c.sym_strs.clear();
    c.sym_strs.resize(ast.names.len(), None);
    let mut main = c.new_level(0, true, 0);
    main.upvals.push(UpvalDesc {
        in_stack: false,
        index: 0,
        name: "_ENV".into(),
        read_only: false,
    });
    c.levels.push(main);
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
    scratch.open = level::relabel(c.levels);
    Ok((proto, last_target))
}

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
    let mut scratch = CompileScratch::default();
    let source = heap.intern(source_name);
    compile_main(ast, &[], version, source, heap, &mut scratch)
}

/// Per-target plan for `assign_stat`'s two-phase store (snapshot first, then
/// emit RHS, then stores) so a later store cannot reorder around an earlier
/// one's table/key reads (PUC manual §3.3.3).
#[derive(Clone, Copy)]
enum LhsPlan {
    Name(ExprId),
    Indexed { obj: u32, key: SetKey },
}

#[derive(Clone, Copy)]
enum SetKey {
    /// String constant index for OP_SetField (k ≤ 0xFF).
    Field(u32),
    /// Small integer literal for OP_SetI (0..=255).
    Int(u32),
    /// Any other key, pinned in a register for OP_SetTable.
    Reg(u32),
}

struct LocalVar<'a> {
    name: &'a str,
    reg: u32,
    read_only: bool,
    captured: bool,
    /// a named vararg (`...t`) bound as a *virtual* view: `t[k]`/`t.n` reads
    /// compile to OP_VARGIDX (no table). Set only when the pre-scan proved the
    /// vararg is never written / never escapes / is not `_ENV`.
    vararg_virtual: bool,
    /// pc at which the variable became visible (for debug LocVar records)
    start_pc: u32,
    /// a compile-time constant (5.4+): no register (`reg` is meaningless)
    /// and no debug entry; uses take the value
    konst: Option<CtConst>,
}

/// One entry in the function's ordered active-variable sequence used for
/// goto/label scope checks. Mirrors PUC's `actvar` list: every local AND every
/// `global` declaration appends one, so a goto that jumps over either lands
/// "into its scope". `reg` is `Some` only for real locals (used to compute the
/// CLOSE register floor); `name` is `None` for a `global *` marker (reported as
/// `'*'` in scope errors).
struct AVar<'a> {
    name: Option<&'a str>,
    reg: Option<u32>,
    /// a `global` declaration (otherwise a local, which a compile-time
    /// constant is too, without a register)
    global: bool,
}

struct BlockCx {
    first_local: usize,
    /// index into `Level::avars` at block entry (goto-scope truncation point)
    first_avar: usize,
    reg_floor: u32,
    is_loop: bool,
    breaks: Vec<usize>,
    /// 5.4: per entry of `breaks`, the number of active locals at the
    /// `break`, to tell which blocks with upvalues it leaves
    break_levels: Vec<usize>,
    /// 5.4: a `break` left the scope of a local needing a CLOSE (PUC's
    /// goto `close` flag), so the loop's "break" label closes
    break_close: bool,
    /// the pc where the block starts
    start_pc: usize,
    /// visible labels defined in this block
    labels: Vec<LabelDef>,
    /// forward gotos not yet matched to a label
    gotos: Vec<GotoRef>,
    /// explicit `global` declarations in this block (name, read_only)
    gdecls: Vec<(Box<str>, bool)>,
    /// `global [attrib] *` in this block: Some(read_only)
    collective: Option<bool>,
    /// any to-be-closed local declared in this block
    has_tbc: bool,
    /// this block is in the scope of a to-be-closed variable (an explicit
    /// <close> local, or a generic-for's implicit closing value): suppresses
    /// tail calls so the function returns to run __close. Tracked separately
    /// from `has_tbc` so it doesn't perturb CLOSE-instruction emission.
    tbc_scope: bool,
    /// 5.4: the loop body's locals (from this index of `locals`) went out
    /// of scope at this pc, before the loop's per-iteration CLOSE; PUC keeps
    /// the body in a block of its own and removes its variables first
    body_end: Option<(usize, u32)>,
    /// the line of the loop's closing `end`, when known: the CLOSE after a
    /// 5.4 `break` label is emitted there
    end_line: Option<u32>,
}

struct LabelDef {
    name: Box<str>,
    pc: usize,
    /// source line of the label (for "already defined on line N")
    line: u32,
    /// locals active at the label (trailing labels use the block floor)
    nactive: usize,
}

struct GotoRef {
    name: Box<str>,
    jmp_pc: usize,
    line: u32,
    nactive: usize,
}

enum VarKind {
    Local(u32),
    /// a compile-time constant local (5.4+)
    Const(CtConst),
    Upval(u32),
    /// global access; read_only from 5.5 declarations
    Global {
        read_only: bool,
    },
}

/// Where an expression's value currently lives.
#[derive(Clone, Copy)]
enum Exp {
    Nil,
    True,
    False,
    Int(i64),
    Float(f64),
    Const(u32),
    /// value sits in a register (local or materialized temp)
    Reg(u32),
    /// instruction at index has an unassigned A (destination pending)
    Reloc(usize),
    /// comparison not yet materialized
    /// `l` is A, `r` is B (a register, or the biased immediate of `EqI`…
    /// `GeI`, or the constant index of `EqK`), `c` is C
    Cmp {
        op: Op,
        l: u32,
        r: u32,
        c: u32,
    },
    /// open multi-result producer (CALL/VARARG) at `pc`, results from `base`
    Open {
        pc: usize,
        base: u32,
    },
}

struct Compiler<'a> {
    ast: &'a Chunk,
    /// see [`compile_parsed`]
    end_lines: &'a [u32],
    heap: &'a mut Heap,
    version: LuaVersion,
    source: Gc<LuaStr>,
    levels: Vec<Level<'a>>,
    /// emptied vectors of finished functions, for the next function
    pool: Vec<LevelBufs>,
    /// the heap string of each entry of the chunk's names, once made
    sym_strs: Vec<Option<Gc<LuaStr>>>,
    last_line: u32,
    /// When `Some(line)`, every `emit` ignores `last_line` and attributes the
    /// new instruction to `line` instead. PUC infix discharges its left
    /// operand *after* the operator token (so the GET emitted for `b[1]` in
    /// `b[1] + …` lands on the operator's line, not the line of `b[1]`);
    /// luna parses the lhs ahead of time and so cannot defer the emit, but
    /// pinning the line here gives the same trace.
    force_line: Option<u32>,
    /// Compile-time literal interning (PUC's `luaX_newstring` cache): identical
    /// string literals anywhere in the chunk — short *or* long — share one
    /// object, so e.g. `string.format("%p", ...)` reports equal addresses for
    /// equal constants. The runtime interner only dedups short strings, so
    /// only long ones are kept here.
    str_cache: HashMap<Box<[u8]>, Gc<LuaStr>>,
}

impl<'a> Compiler<'a> {
    /// The text of a name in the tree.
    fn nm(&self, n: &Name) -> &'a str {
        self.ast.name(*n)
    }

    /// The bytes of a string literal of the tree.
    fn sb(&self, s: ast::Sym) -> &'a [u8] {
        self.ast.str(s)
    }

    /// The items of a list of the tree.
    fn ls<T: ListItem>(&self, l: List<T>) -> &'a [T] {
        self.ast.list(l)
    }

    /// A level for a new function, in kept vectors when there are some.
    fn new_level(&mut self, num_params: u8, is_vararg: bool, line: u32) -> Level<'a> {
        let bufs = self.pool.pop().unwrap_or_default();
        Level::new(num_params, is_vararg, line, bufs)
    }

    /// The finished function `lvl` on the heap; its vectors are kept.
    fn finish_level(&mut self, lvl: Level<'a>, line: u32, last_line: u32) -> Gc<Proto> {
        let (proto, bufs) = lvl.into_proto(self.source, line, last_line, self.heap);
        self.pool.push(bufs);
        self.heap.adopt_proto(proto)
    }

    /// The `end` line the parser recorded for statement `sid`.
    fn stat_end_line(&self, sid: StatId) -> Option<u32> {
        self.end_lines
            .get(sid.0 as usize)
            .copied()
            .filter(|&l| l != 0)
    }

    fn l(&mut self) -> &mut Level<'a> {
        self.levels.last_mut().expect("no level")
    }

    fn lr(&self) -> &Level<'a> {
        self.levels.last().expect("no level")
    }

    fn err(&self, line: u32, msg: impl Into<String>) -> SyntaxError {
        SyntaxError {
            line,
            msg: msg.into().into_bytes(),
        }
    }
}
