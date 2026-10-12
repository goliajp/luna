//! Recursive-descent parser; grammar and operator priorities follow PUC
//! lparser.c. Statement/expression nesting is depth-limited like PUC's
//! C-stack guard.

use crate::frontend::ast::*;
use crate::frontend::error::SyntaxError;
use crate::frontend::goto_check::{GotoCheck, GotoErr, VarKind};
use crate::frontend::lexer::{Lexed, Lexer, Source, Stream};
use crate::frontend::names::{Names, Sym};
use crate::frontend::span::Span;
use crate::frontend::token::{LexTok, Near, Tok, Token, TokenInfo, near_text};
use crate::runtime::mem::{LVec, MemOwner};
use crate::version::LuaVersion;

mod block;
mod decl;
mod expr;
mod func;
mod global_stat;
mod lists;
mod marks;
mod operators;
mod plumbing;
mod scratch;
mod stat;
mod streams;
mod token_source;
mod upval51;
pub(crate) use marks::{Marks, record_marks, token_at};
use operators::{UNARY_PRIORITY, bin_op_of, bin_priority, un_op_of};
pub(crate) use scratch::ParseScratch;
use scratch::{ListStacks, finish};
pub use streams::parse_tokens;
pub(crate) use streams::{parse_stream, parse_tokens_at_depth};
use token_source::Cur;
pub(crate) use token_source::TokenSource;
use upval51::FnUvSlot;

/// PUC `LUAI_MAXCCALLS` — the parser's nesting cap. PUC sets it to 200 and
/// increments once per `subexpr`/`funcargs`/`simpleexp`/`block`/`statement`
/// call; luna's `enter()` fires on roughly the same surfaces (statement +
/// sub_expr + suffixedexp + block), so the same 200 budget keeps
/// errors.lua's `testrep` baseline — 190 levels compile, 201 hits the wall.
const MAX_DEPTH: u32 = 200;

/// PUC `MAXVARS`: active locals per function.
const MAXVARS: u32 = 200;

/// `(collective attrib, declared names, initializer exprs)` of a declaration.
type DeclList = (Option<Attrib>, List<AttribName>, List<ExprId>);

/// Parse a Lua source chunk for the given dialect into an arena AST
/// ([`Chunk`]).
///
/// `LuaVersion::MacroLua` sources are **not** routed through the macro
/// expander here — this entry point is dialect-agnostic and only sees
/// the raw token stream. The Vm's `eval` path runs the expander
/// transparently for MacroLua; direct callers feed expanded tokens via
/// [`parse_tokens`].
///
/// The tree's vectors come from the system allocator.
pub fn parse(src: &[u8], version: LuaVersion) -> Result<Chunk, SyntaxError> {
    parse_at_depth(src, version, 0).map(|p| p.chunk)
}

/// A parsed chunk with what the public [`Chunk`] has no place for.
pub(crate) struct Parsed {
    pub(crate) chunk: Chunk,
    /// the line of each statement's last token, by `StatId`: PUC attributes
    /// the code it emits once it has read that token (a loop's `end`) to
    /// its line
    pub(crate) end_lines: LVec<u32>,
    /// the lexer's token buffer, kept for the next load
    pub(crate) lex_buf: LVec<u8>,
    /// the parser's list stacks, kept for the next load
    pub(crate) stacks: ListStacks,
    /// the source of a parse that read it piece by piece, where a compile
    /// error is placed (a whole source is the caller's)
    pub(crate) src: Option<LVec<u8>>,
}

/// [`parse`] run by a VM that is `c_depth` C calls deep. PUC's parser
/// counts its nesting on the running thread's `nCcalls`, so a chunk loaded
/// near the C-call limit fails to *parse* (and `require` reports it as an
/// error loading the module) before the call that would run it overflows.
pub(crate) fn parse_at_depth(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
) -> Result<Parsed, SyntaxError> {
    // held here too: the parser's own vectors, dropped before this returns
    // on an error, must not outlive the context
    let mem = MemOwner::system();
    parse_reusing(src, version, c_depth, ParseScratch::new(mem.clone()))
}

/// [`parse_at_depth`] building the tree in the vectors of an earlier parse
/// (see [`ParseScratch`]).
pub(crate) fn parse_reusing(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    mut scratch: ParseScratch,
) -> Result<Parsed, SyntaxError> {
    parse_in_place(src, version, c_depth, &mut scratch)?;
    Ok(scratch.into_parsed())
}

/// [`parse_reusing`] leaving the tree in `scratch` (see
/// [`ParseScratch::parsed`]): a load parses and compiles without moving the
/// tree's few hundred bytes of vector headers from one owner to the next.
pub(crate) fn parse_in_place<'s>(
    src: &'s [u8],
    version: LuaVersion,
    c_depth: u32,
    scratch: &'s mut ParseScratch,
) -> Result<(), SyntaxError> {
    scratch.clear();
    let names = scratch.chunk.names.take();
    let lex = Lexer::interning(src, version, names, scratch.lex_buf.take());
    parse_from_source(
        TokenSource::Lexer(lex),
        version,
        c_depth,
        src.len(),
        scratch,
    )
}

fn parse_from_source<'s>(
    mut lex: TokenSource<'s>,
    version: LuaVersion,
    c_depth: u32,
    src_len: usize,
    scratch: &'s mut ParseScratch,
) -> Result<(), SyntaxError> {
    let cur = match lex.next_token() {
        Ok(cur) => cur,
        Err(e) => return Err(lex.out_of_memory().map_or(e, SyntaxError::from)),
    };
    let ParseScratch {
        chunk,
        end_lines,
        lex_buf,
        stacks,
        streamed,
    } = scratch;
    let mut p = Parser::new(lex, cur, version, c_depth, chunk, end_lines, stacks)?;
    let r = p.main(src_len);
    // a reader whose pieces could not be kept ended the input early:
    // whatever the parse made of it, the load failed for lack of memory
    if let Some(o) = p.lex.out_of_memory() {
        return Err(o.into());
    }
    let (block, end_line) = r?;
    let Parser {
        mut lex,
        chunk,
        stk,
        func_local_count,
        funcs,
        gotos,
        ..
    } = p;
    chunk.names = lex.take_names();
    chunk.block = block;
    chunk.end_line = end_line;
    stk.func_local_count = func_local_count;
    stk.funcs = funcs;
    stk.gotos = gotos;
    *lex_buf = lex.take_buf();
    *streamed = lex.take_streamed();
    Ok(())
}

impl<'s> Parser<'s> {
    /// A parser at the first token `cur` of `lex`, building the tree in
    /// `vecs` (emptied).
    fn new(
        lex: TokenSource<'s>,
        cur: Cur,
        version: LuaVersion,
        c_depth: u32,
        chunk: &'s mut Chunk,
        end_lines: &'s mut LVec<u32>,
        stacks: &'s mut ListStacks,
    ) -> Result<Parser<'s>, SyntaxError> {
        let mem = chunk.mem_owner().mem();
        let mut func_local_count = stacks.func_local_count.take();
        func_local_count.clear();
        // the main chunk is the bottom-most function context (line 0 → main)
        func_local_count.push_or_abort((0, 0, 0));
        let mut funcs = stacks.funcs.take();
        funcs.clear();
        funcs.push_or_abort(FnFlow {
            vararg: true,
            loops: 0,
        });
        let gotos = GotoCheck::new(version, stacks.gotos.take(), mem);
        let mut upval_chain_51 = LVec::new(mem);
        if version <= LuaVersion::Lua51 {
            upval_chain_51.push_or_abort(FnUvSlot::new(mem, 0));
        }
        Ok(Parser {
            lex,
            tok: cur.info,
            tok_char: cur.char,
            tok_sym: cur.sym,
            peeked: None,
            prev_line: 1,
            chunk,
            stk: stacks,
            end_lines,
            depth: c_depth,
            version,
            func_local_count,
            funcs,
            gotos,
            last_line: 1,
            upval_chain_51,
            tok_ix: 0,
            rec: None,
        })
    }

    /// The main chunk: its block and the line of its end.
    fn main(&mut self, src_len: usize) -> Result<(Block, u32), SyntaxError> {
        // typical source has an expression node per dozen bytes or so and a
        // statement per few dozen; starting near that skips most regrowth
        let (n_exprs, n_stats) = (src_len / 16, src_len / 64);
        self.chunk.exprs.reserve_or_abort(n_exprs);
        self.chunk.stats.reserve_or_abort(n_stats);
        self.chunk.stat_lines.reserve_or_abort(n_stats);
        self.end_lines.reserve_or_abort(n_stats);
        if let Some(g) = self.gotos.as_mut() {
            g.enter_function();
        }
        let block = self.block()?;
        if self.tok.tok != Token::Eof {
            return Err(self.error_expected("<eof>"));
        }
        self.close_function()?;
        Ok((block, self.prev_line))
    }
}

struct Parser<'s> {
    lex: TokenSource<'s>,
    tok: LexTok,
    /// The byte behind a placeholder `tok` (see [`Cur`]).
    tok_char: Option<u8>,
    /// the interned name when `tok` is a `Token::Name`
    tok_sym: Sym,
    peeked: Option<Cur>,
    /// Per open function (main chunk first): what `...` and `break` are
    /// checked against while parsing, as PUC does.
    funcs: LVec<FnFlow>,
    /// Gotos (and 5.2-5.4 `break`) are resolved while parsing; see
    /// [`GotoCheck`].
    gotos: Option<GotoCheck>,
    /// PUC `ls->lastline`: where the scanner stood before reading the
    /// current token, i.e. the line the last consumed token ended on.
    last_line: u32,
    /// line of the previously consumed token (for the 5.1 ambiguity check)
    prev_line: u32,
    /// the tree being built (its names stay with the lexer until the end)
    chunk: &'s mut Chunk,
    /// lists being collected, before they are moved into `chunk`
    stk: &'s mut ListStacks,
    /// see [`Parsed::end_lines`]
    end_lines: &'s mut LVec<u32>,
    depth: u32,
    version: LuaVersion,
    /// One entry per function context (main chunk + nested functions): the
    /// running active-local count (PUC `nactvar`), the function's defining
    /// line so the limit error can render "in function at line N", and the
    /// locals declared but not yet in scope (`local a, b` before its `=`
    /// list is parsed). Pushed by
    /// `func_body`, popped on exit. Without parse-time tracking, errors.lua
    /// :775 would race a later structural error (a missing `end`) and lose.
    func_local_count: LVec<(u32, u32, u32)>,
    /// Parse-time upvalue accounting for PUC 5.1 (errors.lua :238). PUC 5.1's
    /// `singlevaraux` resolves each identifier as it parses and stops at
    /// `MAXUPVAL=60`; luna defers name resolution to the compiler so a stack
    /// of 61 nested `function`s with no `end`s reaches `<eof>` first and the
    /// missing-`end` error wins. Tracking declared locals + accumulated
    /// upvalue names per nested function here lets the same 60-deep chain
    /// trip while we are still inside `foo61`'s body, with the offending
    /// function's defining line on the error. Only populated for 5.1 — 5.2+
    /// goes through `_ENV` (which would itself be an upvalue) and 5.5
    /// tolerates a wider cap.
    upval_chain_51: LVec<FnUvSlot>,
    /// the number of `tok` among the tokens read, the first being 0
    tok_ix: u32,
    /// set while parsing again to place a compile error (see [`Marks`])
    rec: Option<Box<marks::Recording>>,
}

pub(crate) struct FnFlow {
    vararg: bool,
    /// Loops enclosing the current position inside this function.
    loops: u32,
}
