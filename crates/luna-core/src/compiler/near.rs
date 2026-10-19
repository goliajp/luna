//! The token PUC's scanner is on while its code generator works, named in
//! terms of the tree: a compile error is reported at that token's line and
//! shows it (`near 'x'`), as PUC's errors from the code generator do.

use super::*;
use crate::frontend::parser::{Found, Marks};

/// A token, relative to a node of the tree (see [`Marks`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TokRef {
    /// the `n`th token after expression `e` (0: the one following it)
    After(ExprId, i32),
    /// expression `e`'s first token, plus `n`
    First(ExprId, i32),
    /// statement `s`'s first token, plus `n`
    StatFirst(StatId, i32),
    /// the `n`th token after statement `s`
    StatAfter(StatId, i32),
    /// the `k`th token statement `s` marked (see `Parser::mark`)
    StatMark(StatId, u8),
    /// the `(` function literal `e` opens its parameters with, plus `n`
    Open(ExprId, i32),
    /// the `k`th token statement `s` marked, plus `n`
    MarkPlus(StatId, u8, i32),
    /// the end of the chunk
    Eof,
    /// before the scanner reads the first token (the main function's
    /// start)
    Start,
    /// the token of the next code generation step: an instruction luna
    /// emits early that PUC emits only once the value is used
    Next,
}

impl TokRef {
    /// The token `n` after this one.
    pub(crate) fn plus(self, n: i32) -> TokRef {
        match self {
            TokRef::After(e, k) => TokRef::After(e, k + n),
            TokRef::First(e, k) => TokRef::First(e, k + n),
            TokRef::StatFirst(s, k) => TokRef::StatFirst(s, k + n),
            TokRef::StatAfter(s, k) => TokRef::StatAfter(s, k + n),
            TokRef::StatMark(s, k) => TokRef::MarkPlus(s, k, n),
            TokRef::MarkPlus(s, k, m) => TokRef::MarkPlus(s, k, m + n),
            TokRef::Open(e, k) => TokRef::Open(e, k + n),
            t => t,
        }
    }

    /// The token's number among the chunk's tokens.
    pub(crate) fn resolve(self, ast: &Chunk, m: &Marks) -> u32 {
        let off = |base: u32, n: i32| base.wrapping_add_signed(n);
        match self {
            TokRef::After(e, n) => off(m.expr_follow[e.0 as usize], n),
            TokRef::First(e, n) => off(first_token(ast, m, e), n),
            TokRef::StatFirst(s, n) => off(m.stat_first[s.0 as usize], n),
            TokRef::StatAfter(s, n) => off(m.stat_follow[s.0 as usize], n),
            TokRef::StatMark(s, k) => m.stat_marks(s)[k as usize],
            TokRef::Eof | TokRef::Next => m.eof,
            TokRef::Start => 0,
            TokRef::Open(e, n) => off(m.expr_open(e).unwrap_or(m.eof), n),
            TokRef::MarkPlus(s, k, n) => off(m.stat_marks(s)[k as usize], n),
        }
    }
}

/// The first token of expression `e`: its leftmost operand's, or the token
/// it opens with.
fn first_token(ast: &Chunk, m: &Marks, mut e: ExprId) -> u32 {
    let mut back = 0;
    loop {
        match *ast.expr(e) {
            Expr::BinOp { lhs, .. } => e = lhs,
            Expr::Index { obj, .. } | Expr::MethodCall { obj, .. } => e = obj,
            Expr::Call { func, .. } => e = func,
            Expr::Paren(inner) => {
                back += 1;
                e = inner;
            }
            Expr::UnOp { operand, .. } => {
                back += 1;
                e = operand;
            }
            // `function` comes before the `(` recorded
            Expr::Function(_) => return m.expr_open(e).expect("recorded") - 1 - back,
            Expr::Table { .. } => return m.expr_open(e).expect("recorded") - back,
            _ => return m.expr_follow[e.0 as usize] - 1 - back,
        }
    }
}

impl Compiler<'_> {
    /// PUC's scanner is now on token `t`.
    #[inline]
    pub(super) fn at(&mut self, t: TokRef) {
        self.cur = t;
        if self.next_pending {
            self.next_pending = false;
            if let Some((_, r)) = &mut self.l().jump_err
                && *r == TokRef::Next
            {
                *r = t;
            }
        }
    }

    /// An error PUC raises with `luaX_syntaxerror`: at the current token,
    /// which it shows.
    #[cold]
    #[inline(never)]
    pub(super) fn near_err(&mut self, msg: impl Into<String>) -> SyntaxError {
        let e = self.err(self.last_line, msg);
        self.err_tok = Some((self.cur, e.msg.clone(), true));
        e
    }

    /// An expression nested deeper than the native stack left can compile
    /// (a long left-associative chain such as `1 + 1 + ... + 1`), reported
    /// in the words the dialect's parser uses at its nesting limit.
    #[cold]
    #[inline(never)]
    pub(super) fn too_deep(&self) -> SyntaxError {
        match self.version {
            LuaVersion::Lua51 => self.err(self.last_line, "chunk has too many syntax levels"),
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                let where_ = if self.depth == 1 {
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

    #[cold]
    #[inline(never)]
    pub(super) fn err(&self, line: u32, msg: impl Into<String>) -> SyntaxError {
        SyntaxError {
            line,
            msg: msg.into().into_bytes(),
        }
    }
}

/// `e`, which compiling `ast`, parsed from `src` by a load `c_depth` C
/// calls deep, returned, at the token PUC raises it at: that token's line,
/// shown after the message (`near 'x'`).
pub(crate) fn place_error(
    scratch: &mut CompileScratch,
    ast: &Chunk,
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    e: SyntaxError,
) -> SyntaxError {
    let Some((t, true)) = scratch.error_token(&e) else {
        return e;
    };
    let lenient = scratch.replay.lenient;
    let marks = crate::frontend::parser::record_marks(src, version, c_depth, lenient);
    let ix = t.resolve(ast, &marks.expect("the source parsed before"));
    let at = crate::frontend::parser::token_at(src, version, c_depth, ix, lenient);
    let at = at.expect("a token of the source");
    scratch.replay.at = Some((ix, at.kind));
    shown_at(e, true, &at)
}

/// `e` raised on token `at`: on its line, and showing it when `near`.
pub(crate) fn shown_at(mut e: SyntaxError, near: bool, at: &Found) -> SyntaxError {
    if near {
        e.line = at.line;
        e.msg.extend_from_slice(b" near ");
        e.msg.extend_from_slice(&at.near);
    }
    e
}

/// The error that compiling what a parse of `src` read before failing at
/// `failed` raises on a token up to that one: an error PUC, generating code
/// as it parses, meets before the syntax error (see
/// [`crate::frontend::parser::completed`]).
pub(crate) fn error_before(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    failed: crate::frontend::parser::Failed,
    source: Gc<LuaStr>,
    heap: &mut Heap,
    scratch: &mut CompileScratch,
) -> Option<SyntaxError> {
    use crate::frontend::parser as p;
    let fixed = p::completed(src, version, c_depth, failed, false)?;
    let mem = crate::runtime::mem::MemOwner::system();
    let mut tree = p::ParseScratch::new(mem.clone());
    p::parse_in_place(&fixed, version, c_depth, &mut tree).ok()?;
    let (ast, end_lines) = tree.parsed();
    let e = super::compile_parsed(ast, end_lines, version, source, heap, scratch).err()?;
    let (t, near) = scratch.error_token(&e)?;
    let ix = t.resolve(ast, &p::record_marks(&fixed, version, c_depth, false).ok()?);
    if ix > failed.at {
        return None;
    }
    Some(shown_at(
        e,
        near,
        &p::token_at(src, version, c_depth, ix, false)?,
    ))
}
