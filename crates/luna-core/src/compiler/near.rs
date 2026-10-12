//! The token PUC's scanner is on while its code generator works, named in
//! terms of the tree: a compile error is reported at that token's line and
//! shows it (`near 'x'`), as PUC's errors from the code generator do.

use super::*;
use crate::frontend::parser::Marks;

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
    /// the end of the chunk
    Eof,
    /// the token of the next code generation step: an instruction luna
    /// emits early that PUC emits only once the value is used
    Next,
}

impl TokRef {
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
        self.err_tok = Some((self.cur, e.msg.clone()));
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
    mut e: SyntaxError,
) -> SyntaxError {
    let Some(t) = scratch.error_token(&e) else {
        return e;
    };
    let reparse = "the source parsed before";
    let marks = crate::frontend::parser::record_marks(src, version, c_depth).expect(reparse);
    let ix = t.resolve(ast, &marks);
    let (line, near) = crate::frontend::parser::token_at(src, version, c_depth, ix)
        .expect(reparse)
        .expect("a token of the source");
    e.line = line;
    e.msg.extend_from_slice(b" near ");
    e.msg.extend_from_slice(&near);
    e
}
