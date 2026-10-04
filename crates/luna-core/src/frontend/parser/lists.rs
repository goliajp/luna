//! The parts of the grammar that read lists: expression lists, call
//! arguments and table constructors.

use super::*;

impl Parser<'_> {
    pub(super) fn exprlist(&mut self) -> Result<List<ExprId>, SyntaxError> {
        let mark = self.stk.exprs.len();
        let e = self.expr()?;
        self.stk.exprs.push(e);
        while self.accept(Token::Comma)? {
            let e = self.expr()?;
            self.stk.exprs.push(e);
        }
        Ok(finish(&mut self.chunk, &mut self.stk.exprs, mark))
    }

    pub(super) fn call_args(&mut self) -> Result<List<ExprId>, SyntaxError> {
        match &self.tok.tok {
            Token::LParen => {
                // 5.1 rejects a call paren on a new line (removed in 5.2)
                if self.version == LuaVersion::Lua51 && self.tok.line != self.prev_line {
                    return Err(self.error("ambiguous syntax (function call x new statement)"));
                }
                let line = self.tok.line;
                self.advance()?;
                let args = if self.tok.tok == Token::RParen {
                    List::EMPTY
                } else {
                    self.exprlist()?
                };
                self.expect_match(Token::RParen, ")", "(", line)?;
                Ok(args)
            }
            Token::Str(_) => {
                let s = self.tok_sym;
                self.advance()?;
                let e = self.push_expr(Expr::Str(s));
                Ok(self.chunk.push_list(&[e]))
            }
            Token::LBrace => {
                let e = self.table_constructor()?;
                Ok(self.chunk.push_list(&[e]))
            }
            _ => Err(self.error("function arguments expected")),
        }
    }

    pub(super) fn table_constructor(&mut self) -> Result<ExprId, SyntaxError> {
        let line = self.tok.line;
        self.expect(Token::LBrace, "{")?;
        let mark = self.stk.fields.len();
        loop {
            if self.tok.tok == Token::RBrace {
                break;
            }
            if self.tok.tok == Token::LBracket {
                self.advance()?;
                let key = self.expr()?;
                self.expect(Token::RBracket, "]")?;
                self.expect(Token::Assign, "=")?;
                let value = self.expr()?;
                self.stk.fields.push(TableField::Keyed(key, value));
            } else if matches!(self.tok.tok, Token::Name(_)) && *self.peek()? == Token::Assign {
                let name = self.expect_name()?;
                self.advance()?; // '='
                let value = self.expr()?;
                self.stk.fields.push(TableField::Named(name, value));
            } else {
                let e = self.expr()?;
                self.stk.fields.push(TableField::Item(e));
            }
            if !(self.accept(Token::Comma)? || self.accept(Token::Semi)?) {
                break;
            }
        }
        self.expect_match(Token::RBrace, "}", "{", line)?;
        let fields = finish(&mut self.chunk, &mut self.stk.fields, mark);
        Ok(self.push_expr(Expr::Table { fields, line }))
    }

    /// Declare a declaration's names to the goto checker: locals, read-only
    /// when they or the whole list (`collective`) have an attribute, or 5.5
    /// globals.
    pub(super) fn declare_attrib_names(
        &mut self,
        names: List<AttribName>,
        collective: Option<Attrib>,
        global: bool,
    ) {
        let Parser {
            gotos, lex, chunk, ..
        } = self;
        if let Some(g) = gotos {
            for an in chunk.list(names) {
                let kind = match an.attrib.or(collective) {
                    _ if global => VarKind::Global,
                    Some(_) => VarKind::Const,
                    None => VarKind::Local,
                };
                g.declare(lex.names().text(an.name.sym), kind);
            }
        }
    }
}
