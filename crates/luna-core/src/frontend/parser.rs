//! Recursive-descent parser; grammar and operator priorities follow PUC
//! lparser.c. Statement/expression nesting is depth-limited like PUC's
//! C-stack guard.

use crate::frontend::ast::*;
use crate::frontend::error::SyntaxError;
use crate::frontend::goto_check::{GotoCheck, VarKind};
use crate::frontend::lexer::{Lexed, Lexer, Source, Stream};
use crate::frontend::names::{Names, Sym};
use crate::frontend::span::Span;
use crate::frontend::token::{LexTok, Near, Tok, Token, TokenInfo, near_text};
use crate::version::LuaVersion;

mod block;
mod expr;
mod func;
mod lists;
mod plumbing;
mod scratch;
mod stat;
mod token_source;
mod upval51;
pub(crate) use scratch::ParseScratch;
use scratch::{ListStacks, finish};
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

/// Binary operator priorities from lparser.c (left, right); right < left
/// means right-associative.
fn bin_priority(op: BinOp) -> (u8, u8) {
    match op {
        BinOp::Or => (1, 1),
        BinOp::And => (2, 2),
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Ne | BinOp::Eq => (3, 3),
        BinOp::BOr => (4, 4),
        BinOp::BXor => (5, 5),
        BinOp::BAnd => (6, 6),
        BinOp::Shl | BinOp::Shr => (7, 7),
        BinOp::Concat => (9, 8),
        BinOp::Add | BinOp::Sub => (10, 10),
        BinOp::Mul | BinOp::Div | BinOp::IDiv | BinOp::Mod => (11, 11),
        BinOp::Pow => (14, 13),
    }
}

const UNARY_PRIORITY: u8 = 12;

fn bin_op_of(tok: &Tok) -> Option<BinOp> {
    Some(match tok {
        Token::Plus => BinOp::Add,
        Token::Minus => BinOp::Sub,
        Token::Star => BinOp::Mul,
        Token::Slash => BinOp::Div,
        Token::DSlash => BinOp::IDiv,
        Token::Percent => BinOp::Mod,
        Token::Caret => BinOp::Pow,
        Token::Concat => BinOp::Concat,
        Token::Eq => BinOp::Eq,
        Token::Ne => BinOp::Ne,
        Token::Lt => BinOp::Lt,
        Token::Le => BinOp::Le,
        Token::Gt => BinOp::Gt,
        Token::Ge => BinOp::Ge,
        Token::And => BinOp::And,
        Token::Or => BinOp::Or,
        Token::Amp => BinOp::BAnd,
        Token::Pipe => BinOp::BOr,
        Token::Tilde => BinOp::BXor,
        Token::Shl => BinOp::Shl,
        Token::Shr => BinOp::Shr,
        _ => return None,
    })
}

fn un_op_of(tok: &Tok) -> Option<UnOp> {
    Some(match tok {
        Token::Minus => UnOp::Neg,
        Token::Not => UnOp::Not,
        Token::Hash => UnOp::Len,
        Token::Tilde => UnOp::BNot,
        _ => return None,
    })
}

/// Parse a Lua source chunk for the given dialect into an arena AST
/// ([`Chunk`]).
///
/// `LuaVersion::MacroLua` sources are **not** routed through the macro
/// expander here — this entry point is dialect-agnostic and only sees
/// the raw token stream. The Vm's `eval` path runs the expander
/// transparently for MacroLua; direct callers feed expanded tokens via
/// [`parse_tokens`].
pub fn parse(src: &[u8], version: LuaVersion) -> Result<Chunk, SyntaxError> {
    parse_at_depth(src, version, 0).map(|p| p.chunk)
}

/// A parsed chunk with what the public [`Chunk`] has no place for.
pub(crate) struct Parsed {
    pub(crate) chunk: Chunk,
    /// the line of the closing `end` of each `while` / `for` statement, by
    /// `StatId` (0 for other statements): PUC attributes the code it emits
    /// after reading that `end` to its line
    pub(crate) end_lines: Vec<u32>,
    /// the lexer's token buffer, kept for the next load
    pub(crate) lex_buf: Vec<u8>,
    /// the parser's list stacks, kept for the next load
    pub(crate) stacks: ListStacks,
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
    parse_reusing(src, version, c_depth, ParseScratch::default())
}

/// [`parse_at_depth`] building the tree in the vectors of an earlier parse
/// (see [`ParseScratch`]).
pub(crate) fn parse_reusing(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    scratch: ParseScratch,
) -> Result<Parsed, SyntaxError> {
    let ParseScratch {
        mut chunk,
        end_lines,
        lex_buf,
        stacks,
    } = scratch;
    let names = std::mem::take(&mut chunk.names);
    let lex = Lexer::interning(src, version, names, lex_buf);
    parse_from_source(
        TokenSource::Lexer(lex),
        version,
        c_depth,
        src.len(),
        (chunk, end_lines, stacks),
    )
}

/// [`parse_reusing`] over a source read piece by piece: `first` is the
/// piece already read, and `feed` appends the next one or returns false at
/// the end. The parser asks for a piece only when the scan moves past the
/// bytes it has, as PUC's does, so it stops reading at a syntax error.
pub(crate) fn parse_stream<'f>(
    first: Vec<u8>,
    feed: &'f mut crate::frontend::lexer::Feed<'f>,
    version: LuaVersion,
    c_depth: u32,
    scratch: ParseScratch,
) -> Result<Parsed, SyntaxError> {
    let ParseScratch {
        mut chunk,
        end_lines,
        lex_buf,
        stacks,
    } = scratch;
    let names = std::mem::take(&mut chunk.names);
    let len = first.len();
    let lex = Lexer::interning_stream(Stream::new(first, feed), version, names, lex_buf);
    parse_from_source(
        TokenSource::Stream(lex),
        version,
        c_depth,
        len,
        (chunk, end_lines, stacks),
    )
}

/// Parse a **pre-materialized** token stream. Used by the MacroLua
/// expander pre-pass — it walks the lexer output once, expands
/// `@name(...)` invocations against the per-Vm macro registry, and
/// feeds the resulting `Vec<TokenInfo>` here.
pub fn parse_tokens(
    tokens: Vec<TokenInfo>,
    src: &[u8],
    version: LuaVersion,
) -> Result<Chunk, SyntaxError> {
    parse_tokens_at_depth(tokens, src, version, 0).map(|p| p.chunk)
}

/// [`parse_tokens`] at a C depth (see [`parse_at_depth`]).
pub(crate) fn parse_tokens_at_depth(
    tokens: Vec<TokenInfo>,
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
) -> Result<Parsed, SyntaxError> {
    parse_from_source(
        TokenSource::PreExpanded {
            tokens,
            cursor: 0,
            src,
            names: Names::with_capacity(src.len()),
        },
        version,
        c_depth,
        src.len(),
        Default::default(),
    )
}

fn parse_from_source<'s>(
    mut lex: TokenSource<'s>,
    version: LuaVersion,
    c_depth: u32,
    src_len: usize,
    vecs: (Chunk, Vec<u32>, ListStacks),
) -> Result<Parsed, SyntaxError> {
    let (mut chunk, mut end_lines, mut stacks) = vecs;
    let mut func_local_count = std::mem::take(&mut stacks.func_local_count);
    func_local_count.clear();
    // the main chunk is the bottom-most function context (line 0 → main)
    func_local_count.push((0, 0, 0));
    let mut funcs = std::mem::take(&mut stacks.funcs);
    funcs.clear();
    funcs.push(FnFlow {
        vararg: true,
        loops: 0,
    });
    let gotos = GotoCheck::new(version, stacks.gotos.take());
    let cur = lex.next_token()?;
    // typical source has an expression node per dozen bytes or so and a
    // statement per few dozen; starting near that skips most regrowth
    let (n_exprs, n_stats) = (src_len / 16, src_len / 64);
    chunk.exprs.reserve(n_exprs);
    chunk.stats.reserve(n_stats);
    chunk.stat_lines.reserve(n_stats);
    end_lines.reserve(n_stats);
    let mut p = Parser {
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
        upval_chain_51: if version <= LuaVersion::Lua51 {
            vec![FnUvSlot {
                line_defined: 0,
                ..Default::default()
            }]
        } else {
            Vec::new()
        },
    };
    if let Some(g) = p.gotos.as_mut() {
        g.enter_function();
    }
    let block = p.block()?;
    if p.tok.tok != Token::Eof {
        return Err(p.error_expected("<eof>"));
    }
    p.close_function()?;
    let end_line = p.prev_line;
    let mut chunk = p.chunk;
    chunk.names = p.lex.take_names();
    let mut stacks = p.stk;
    stacks.func_local_count = p.func_local_count;
    stacks.funcs = p.funcs;
    stacks.gotos = p.gotos;
    chunk.block = block;
    chunk.end_line = end_line;
    Ok(Parsed {
        lex_buf: p.lex.take_buf(),
        chunk,
        end_lines: p.end_lines,
        stacks,
    })
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
    funcs: Vec<FnFlow>,
    /// Gotos (and 5.2-5.4 `break`) are resolved while parsing; see
    /// [`GotoCheck`].
    gotos: Option<GotoCheck>,
    /// PUC `ls->lastline`: where the scanner stood before reading the
    /// current token, i.e. the line the last consumed token ended on.
    last_line: u32,
    /// line of the previously consumed token (for the 5.1 ambiguity check)
    prev_line: u32,
    /// the tree being built (its names stay with the lexer until the end)
    chunk: Chunk,
    /// lists being collected, before they are moved into `chunk`
    stk: ListStacks,
    /// see [`Parsed::end_lines`]
    end_lines: Vec<u32>,
    depth: u32,
    version: LuaVersion,
    /// One entry per function context (main chunk + nested functions): the
    /// running active-local count (PUC `nactvar`), the function's defining
    /// line so the limit error can render "in function at line N", and the
    /// locals declared but not yet in scope (`local a, b` before its `=`
    /// list is parsed). Pushed by
    /// `func_body`, popped on exit. Without parse-time tracking, errors.lua
    /// :775 would race a later structural error (a missing `end`) and lose.
    func_local_count: Vec<(u32, u32, u32)>,
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
    upval_chain_51: Vec<FnUvSlot>,
}

pub(crate) struct FnFlow {
    vararg: bool,
    /// Loops enclosing the current position inside this function.
    loops: u32,
}
