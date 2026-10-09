//! AST → bytecode compiler. Register model follows PUC lparser/lcode:
//! locals pin the low registers, temporaries grow from `freereg`, constants
//! are deduplicated, forward jumps are patch lists (plain Vecs instead of
//! PUC's in-code jump chains). Function nesting is a stack of `Level`s;
//! upvalue resolution walks it (PUC singlevaraux).
//!
//! Slice 3 state: calls, closures, upvalues, varargs (5.5 table semantics),
//! generic `for`, multret, tail calls. Still pending (slice 5): goto/labels,
//! `<close>`, `global` declarations.

mod assign;
mod assign_conflict;
mod binop;
mod binop_classic;
mod binop_const;
mod binop_emit;
mod binop_eq;
mod closure;
mod cond;
pub(crate) mod const_map;
mod control;
mod ctconst;
mod discharge;
mod emit;
mod expr;
mod expr_names;
mod expr_ops;
mod fold;
mod for_loops;
mod jumplist;
mod labels;
use jumplist::NO_JUMP;
mod level;
mod limits;
mod lvalue;
mod main_fn;
pub use main_fn::compile_chunk_with_last_target;
use main_fn::compile_main;
mod resolve;
mod return_stat;
mod scope;
mod stat;
mod table_ctor;
mod vararg_scan;
use const_map::ConstMap;
use ctconst::{CtConst, ct_operand, ct_value};
use fold::{fold_arith, is_logical, numeral};
pub(crate) use level::CompileScratch;
use level::{Level, LevelBufs};
use limits::{MAX_LOCALS, max_regs, max_upvals};
use lvalue::{KeyRef, Lv, TabRef};

use crate::frontend::ast::{
    self, AttribName, BinOp, Block, Chunk, Expr, ExprId, FuncBody, FuncName, List, ListItem, Name,
    Stat, StatId, TableField, UnOp, block_uses_vararg,
};
use crate::frontend::error::SyntaxError;
use crate::numeric::Num;
use crate::runtime::heap::{GcHeader, ObjTag};
use crate::runtime::mem::{LMap, LVec};
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
    let mut scratch = CompileScratch::new(heap.mem());
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

struct BlockCx<'a> {
    first_local: usize,
    /// index into `Level::avars` at block entry (PUC `bl->nactvar`)
    first_avar: usize,
    reg_floor: u32,
    /// PUC `bl->isloop`; 5.5 sets 2 once a `break` waits for the loop
    is_loop: u8,
    /// 5.1: the loop's `break` jumps (PUC `bl->breaklist`)
    breaklist: i32,
    /// the first of `Level::labels` / `Level::gotos` this block holds
    first_label: usize,
    first_goto: usize,
    /// explicit `global` declarations in this block (name, read_only)
    gdecls: LVec<(&'a str, bool)>,
    /// `global [attrib] *` in this block: Some(read_only)
    collective: Option<bool>,
    /// any to-be-closed local declared in this block
    has_tbc: bool,
    /// this block is in the scope of a to-be-closed variable (an explicit
    /// <close> local, or a generic-for's implicit closing value): suppresses
    /// tail calls so the function returns to run __close. Tracked separately
    /// from `has_tbc` so it doesn't perturb CLOSE-instruction emission.
    tbc_scope: bool,
}

/// A label, or a pending `goto` (PUC `Labeldesc`).
#[derive(Clone, Copy)]
struct LabelDesc<'a> {
    name: &'a str,
    /// the label's pc; a goto's jump list
    pc: i32,
    /// active variables at the label or goto (PUC `nactvar`)
    nactvar: usize,
    /// 5.4+: the goto leaves a block that needs closing
    close: bool,
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
    /// a test and its jump, the jump at this pc taken when the test is
    /// true (PUC `VJMP`)
    Jmp(usize),
    /// a value with true / false jump lists: entry of `Level::jexps`
    Jumps(u32),
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
    levels: LVec<Level<'a>>,
    /// emptied vectors of finished functions, for the next function
    pool: LVec<LevelBufs>,
    /// the heap string of each entry of the chunk's names, once made
    sym_strs: LVec<Option<Gc<LuaStr>>>,
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
    str_cache: LMap<Gc<LuaStr>, Gc<LuaStr>>,
    /// the left spines of the expressions being compiled (see [`Self::expr`])
    spine: LVec<expr::Pending>,
    /// an assignment's targets, taken while one is compiled
    lvs: LVec<lvalue::Lv>,
    /// a concatenation's operands, taken while one is compiled
    operands: LVec<ExprId>,
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
        let bufs = match self.pool.pop() {
            Some(b) => b,
            None => LevelBufs::new(self.heap.mem()),
        };
        Level::new(num_params, is_vararg, line, bufs)
    }

    /// The finished function `lvl` on the heap; its vectors are kept.
    fn finish_level(&mut self, mut lvl: Level<'a>, line: u32, last_line: u32) -> Gc<Proto> {
        if self.version <= LuaVersion::Lua53 {
            for i in lvl.code.iter_mut() {
                *i = crate::vm::isa::imm_form::to_imm(*i, &lvl.consts);
            }
        }
        let (proto, bufs) = lvl.into_proto(self.source, line, last_line, self.heap);
        self.pool.push_or_abort(bufs);
        self.heap.adopt_proto(proto)
    }

    /// The line of statement `sid`'s last token, as the parser recorded it.
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

    /// An expression nested deeper than the native stack left can compile
    /// (a long left-associative chain such as `1 + 1 + ... + 1`), reported
    /// in the words the dialect's parser uses at its nesting limit.
    fn too_deep(&self) -> SyntaxError {
        match self.version {
            LuaVersion::Lua51 => self.err(self.last_line, "chunk has too many syntax levels"),
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                let where_ = if self.levels.len() == 1 {
                    "main function".to_string()
                } else {
                    format!("function at line {}", self.lr().line_defined)
                };
                let msg = format!("too many C levels (limit is 200) in {where_}");
                self.err(self.last_line, msg)
            }
            _ => SyntaxError::unpositioned("C stack overflow"),
        }
    }

    fn err(&self, line: u32, msg: impl Into<String>) -> SyntaxError {
        SyntaxError {
            line,
            msg: msg.into().into_bytes(),
        }
    }
}
