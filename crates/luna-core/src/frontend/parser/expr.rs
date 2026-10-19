//! Expressions: operator precedence, simple, primary and suffixed expressions.

use super::*;

impl<'s> Parser<'s> {
    pub(super) fn expr(&mut self) -> Result<ExprId, SyntaxError> {
        self.sub_expr(0)
    }

    pub(super) fn sub_expr(&mut self, limit: u8) -> Result<ExprId, SyntaxError> {
        self.enter()?;
        let mut left = if let Some(op) = un_op_of(&self.tok.tok) {
            let line = self.tok.line;
            self.advance()?;
            let operand = self.sub_expr(UNARY_PRIORITY)?;
            self.push_expr(Expr::UnOp { op, operand, line })
        } else {
            self.simple_expr()?
        };
        while let Some(op) = bin_op_of(&self.tok.tok) {
            let (lp, rp) = bin_priority(op);
            if lp <= limit {
                break;
            }
            let line = self.tok.line;
            self.advance()?;
            let rhs = self.sub_expr(rp)?;
            if matches!(op, BinOp::Pow | BinOp::Mod) {
                let stamp = crate::cerrno::writes();
                self.chunk.fold_marks.push_or_abort((rhs.0, stamp));
            }
            left = self.push_expr(Expr::BinOp {
                op,
                lhs: left,
                rhs,
                line,
            });
        }
        self.leave();
        Ok(left)
    }

    pub(super) fn simple_expr(&mut self) -> Result<ExprId, SyntaxError> {
        let e = match &self.tok.tok {
            Token::Nil => {
                self.advance()?;
                Expr::Nil
            }
            Token::True => {
                self.advance()?;
                Expr::True
            }
            Token::False => {
                self.advance()?;
                Expr::False
            }
            Token::Ellipsis => {
                if !self.funcs.last().expect("func ctx").vararg {
                    return Err(self.error("cannot use '...' outside a vararg function"));
                }
                self.advance()?;
                Expr::Vararg
            }
            &Token::Int(v) => {
                self.advance()?;
                Expr::Int(v)
            }
            &Token::Float(v) => {
                self.advance()?;
                Expr::Float(v)
            }
            Token::Str(_) => {
                let s = self.tok_sym;
                self.advance()?;
                Expr::Str(s)
            }
            Token::LBrace => return self.table_constructor(),
            Token::Function => {
                let line = self.tok.line;
                self.advance()?;
                let open = self.tok_ix;
                let e = Expr::Function(self.func_body(line)?);
                let id = self.push_expr(e);
                self.opened_expr(id, open);
                return Ok(id);
            }
            _ => return self.suffixed_expr(),
        };
        Ok(self.push_expr(e))
    }

    pub(super) fn primary_expr(&mut self) -> Result<ExprId, SyntaxError> {
        match &self.tok.tok {
            Token::Name(_) => {
                let name = self.expect_name()?;
                self.ident_lookup_51(name.sym)?;
                Ok(self.push_expr(Expr::Name(name)))
            }
            Token::LParen => {
                let line = self.tok.line;
                self.advance()?;
                let inner = self.expr()?;
                self.expect_match(Token::RParen, ")", "(", line)?;
                Ok(self.push_expr(Expr::Paren(inner)))
            }
            _ => Err(self.error("unexpected symbol")),
        }
    }

    pub(super) fn suffixed_expr(&mut self) -> Result<ExprId, SyntaxError> {
        // PUC's `suffixedexp` does *not* bump `nCcalls` on its own — only its
        // callers do (subexpr, funcargs, …). Doing so here too would
        // double-count `(` nesting, since `simpleexp`'s default branch dives
        // through `suffixed_expr` → `primary_expr` → `expr()` → `sub_expr`
        // (which already enters). Keeping the entry out lets errors.lua's
        // `testrep("(")` (paren-only nesting) hit the same 200-level wall as
        // `{`/`,` nesting.
        // PUC 5.2–5.3 `suffixedexp` captured the line of the *primary*
        // expression once and pinned every chained call/method to it
        // (`a\n(...)` → error reports on `a`'s line); 5.1's `funcargs` and
        // 5.4 on take the line of the token the arguments open with. errors.lua's `lineerror` covers both:
        //   - 5.3:  `lineerror([[a\n(\n23)]], 1)` — expects `a`'s line
        //   - 5.4+: `lineerror([[a\n(...)\n23)]], 2)` — expects `(`'s line
        let primary_line = self.tok.line;
        let mut e = self.primary_expr()?;
        loop {
            match &self.tok.tok {
                Token::Dot => {
                    self.advance()?;
                    let name = self.expect_name()?;
                    let key = self.push_expr(Expr::Str(name.sym));
                    self.chunk.dot_keys.push_or_abort(key.0);
                    e = self.push_expr(Expr::Index { obj: e, key });
                }
                Token::LBracket => {
                    self.advance()?;
                    let key = self.expr()?;
                    self.expect(Token::RBracket, "]")?;
                    e = self.push_expr(Expr::Index { obj: e, key });
                }
                Token::Colon => {
                    self.advance()?;
                    let method = self.expect_name()?;
                    let line = if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
                        primary_line
                    } else {
                        self.tok.line
                    };
                    let args = self.call_args()?;
                    e = self.push_expr(Expr::MethodCall {
                        obj: e,
                        method,
                        args,
                        line,
                    });
                }
                Token::LParen | Token::Str(_) | Token::LBrace => {
                    let line = if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
                        primary_line
                    } else {
                        self.tok.line
                    };
                    let args = self.call_args()?;
                    e = self.push_expr(Expr::Call {
                        func: e,
                        args,
                        line,
                    });
                }
                _ => break,
            }
        }
        Ok(e)
    }
}
