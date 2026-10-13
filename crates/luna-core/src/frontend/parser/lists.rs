//! The parts of the grammar that read lists: expression lists, call
//! arguments and table constructors.

use super::*;

/// PUC 5.4+ cap on the items of one table constructor (`INT_MAX / 2`;
/// 5.5's `MAX_CNST` works out to the same value).
const MAX_CTOR_ITEMS: usize = (i32::MAX / 2) as usize;

/// Whether a constructor that has read `items` items is past the cap. 5.1
/// to 5.3 compare `int` counters with `INT_MAX`, a check that cannot fail,
/// so those dialects have no cap.
fn ctor_items_over(version: LuaVersion, items: usize) -> bool {
    version >= LuaVersion::Lua54 && items > MAX_CTOR_ITEMS
}

/// PUC `errorlimit`'s message for the constructor cap, in `where_`.
fn ctor_limit_msg(where_: &str) -> String {
    format!("too many items in a constructor (limit is {MAX_CTOR_ITEMS}) in {where_}")
}

impl Parser<'_> {
    pub(super) fn exprlist(&mut self) -> Result<List<ExprId>, SyntaxError> {
        let mark = self.stk.exprs.len();
        let e = self.expr()?;
        self.stk.exprs.push_or_abort(e);
        while self.accept(Token::Comma)? {
            let e = self.expr()?;
            self.stk.exprs.push_or_abort(e);
        }
        Ok(finish(self.chunk, &mut self.stk.exprs, mark))
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
        let open = self.tok_ix;
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
                self.stk.fields.push_or_abort(TableField::Keyed(key, value));
            } else if matches!(self.tok.tok, Token::Name(_)) && *self.peek()? == Token::Assign {
                let name = self.expect_name()?;
                self.advance()?; // '='
                let value = self.expr()?;
                self.stk
                    .fields
                    .push_or_abort(TableField::Named(name, value));
            } else {
                let e = self.expr()?;
                self.stk.fields.push_or_abort(TableField::Item(e));
            }
            // checked after each item, before its separator: the error is
            // near the token that follows the item
            if ctor_items_over(self.version, self.stk.fields.len() - mark) {
                return Err(self.error(ctor_limit_msg(&self.where_())));
            }
            if !(self.accept(Token::Comma)? || self.accept(Token::Semi)?) {
                break;
            }
        }
        self.expect_match(Token::RBrace, "}", "{", line)?;
        let fields = finish(self.chunk, &mut self.stk.fields, mark);
        let id = self.push_expr(Expr::Table { fields, line });
        self.opened_expr(id, open);
        Ok(id)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_cap_is_puc_5_4_int_max_half() {
        for v in [LuaVersion::Lua54, LuaVersion::MacroLua, LuaVersion::Lua55] {
            assert!(!ctor_items_over(v, 1_073_741_823));
            assert!(ctor_items_over(v, 1_073_741_824));
        }
    }

    #[test]
    fn constructors_have_no_cap_before_5_4() {
        // PUC 5.1-5.3 compare int counters with INT_MAX, which never fails
        for v in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53] {
            assert!(!ctor_items_over(v, 1_073_741_824));
            assert!(!ctor_items_over(v, usize::MAX));
        }
    }

    #[test]
    fn constructor_cap_message_matches_puc() {
        // PUC 5.4.9 and 5.5.1 on 1073741824 `nil,` items read through load:
        // ctor:1: too many items in a constructor (limit is 1073741823) in main function near ','
        assert_eq!(
            ctor_limit_msg("main function"),
            "too many items in a constructor (limit is 1073741823) in main function"
        );
    }
}
