//! Compound statements, declarations and expression statements.

use super::*;

impl<'s> Parser<'s> {
    pub(super) fn if_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let mark = self.stk.arms.len();
        let cond = self.expr()?;
        let then_line = self.tok.line;
        self.expect(Token::Then, "then")?;
        let body = self.block()?;
        self.stk.arms.push_or_abort(IfArm {
            cond,
            then_line,
            body,
        });
        while self.tok.tok == Token::Elseif {
            self.advance()?;
            let cond = self.expr()?;
            let then_line = self.tok.line;
            self.expect(Token::Then, "then")?;
            let body = self.block()?;
            self.stk.arms.push_or_abort(IfArm {
                cond,
                then_line,
                body,
            });
        }
        let arms = finish(&mut self.chunk, &mut self.stk.arms, mark);
        let else_body = if self.accept(Token::Else)? {
            Some(self.block()?)
        } else {
            None
        };
        self.expect_match(Token::End, "end", "if", line)?;
        Ok(self.push_stat(Stat::If { arms, else_body }))
    }

    pub(super) fn while_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let cond = self.expr()?;
        self.expect(Token::Do, "do")?;
        let body = self.loop_block(List::EMPTY)?;
        self.expect_match(Token::End, "end", "while", line)?;
        Ok(self.push_stat(Stat::While { cond, body }))
    }

    pub(super) fn repeat_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let body = self.loop_block(List::EMPTY)?;
        self.expect_match(Token::Until, "until", "repeat", line)?;
        let cond = self.expr()?;
        Ok(self.push_stat(Stat::Repeat { body, cond }))
    }

    pub(super) fn for_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let first = self.expect_name()?;
        match self.tok.tok {
            Token::Assign => {
                self.advance()?;
                // the loop's hidden control locals count against the
                // local limit from the header on (PUC `fornum`): three up
                // to 5.4, two from 5.5
                let hidden = if self.version >= LuaVersion::Lua55 {
                    2
                } else {
                    3
                };
                self.new_locals(hidden + 1)?;
                let start = self.expr()?;
                self.expect(Token::Comma, ",")?;
                let limit = self.expr()?;
                let step = if self.accept(Token::Comma)? {
                    Some(self.expr()?)
                } else {
                    None
                };
                self.expect(Token::Do, "do")?;
                self.add_local_51(first.sym);
                let var = self.chunk.push_list(&[first]);
                let body = self.loop_block(var)?;
                self.expect_match(Token::End, "end", "for", line)?;
                Ok(self.push_stat(Stat::NumericFor {
                    var: first,
                    start,
                    limit,
                    step,
                    body,
                }))
            }
            Token::Comma | Token::In => {
                let mark = self.stk.names.len();
                self.stk.names.push_or_abort(first);
                while self.accept(Token::Comma)? {
                    let n = self.expect_name()?;
                    self.stk.names.push_or_abort(n);
                }
                let vars = finish(&mut self.chunk, &mut self.stk.names, mark);
                // PUC `forlist`: three hidden locals (generator, state,
                // control), four in 5.4 (plus the closing value), three
                // again in 5.5
                let hidden = if self.version == LuaVersion::Lua54 {
                    4
                } else {
                    3
                };
                self.new_locals(hidden + vars.range().len() as u32)?;
                self.expect(Token::In, "in")?;
                let expr_line = self.tok.line;
                let exprs = self.exprlist()?;
                self.expect(Token::Do, "do")?;
                for i in vars.range() {
                    self.add_local_51(self.chunk.name_lists[i].sym);
                }
                let body = self.loop_block(vars)?;
                self.expect_match(Token::End, "end", "for", line)?;
                Ok(self.push_stat(Stat::GenericFor {
                    vars,
                    exprs,
                    body,
                    expr_line,
                }))
            }
            _ => Err(self.error("'=' or 'in' expected")),
        }
    }

    pub(super) fn function_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let base = self.expect_name()?;
        let mark = self.stk.names.len();
        while self.accept(Token::Dot)? {
            let n = self.expect_name()?;
            self.stk.names.push_or_abort(n);
        }
        let path = finish(&mut self.chunk, &mut self.stk.names, mark);
        let method = if self.accept(Token::Colon)? {
            Some(self.expect_name()?)
        } else {
            None
        };
        let body = self.func_body(line)?;
        Ok(self.push_stat(Stat::Function {
            name: FuncName { base, path, method },
            body,
        }))
    }

    pub(super) fn attrib(&mut self) -> Result<Option<Attrib>, SyntaxError> {
        if !(self.version.has_attribs() && self.tok.tok == Token::Lt) {
            return Ok(None);
        }
        self.advance()?;
        let name = self.expect_name()?;
        let attrib = match self.text(name.sym) {
            "const" => Attrib::Const,
            "close" => Attrib::Close,
            other => {
                return Err(SyntaxError {
                    line: name.line,
                    msg: format!("unknown attribute '{other}'").into_bytes(),
                });
            }
        };
        self.expect(Token::Gt, ">")?;
        Ok(Some(attrib))
    }

    /// `[attrib] Name [attrib] {',' Name [attrib]} ['=' explist]` — shared by
    /// `local` and `global` declarations.
    pub(super) fn attnamelist(&mut self) -> Result<DeclList, SyntaxError> {
        let collective = if self.version.has_collective_attrib() {
            self.attrib()?
        } else {
            None
        };
        let mark = self.stk.attribs.len();
        loop {
            let name = self.expect_name()?;
            self.new_local()?;
            let attrib = self.attrib()?;
            self.stk.attribs.push_or_abort(AttribName { name, attrib });
            if !self.accept(Token::Comma)? {
                break;
            }
        }
        let names = finish(&mut self.chunk, &mut self.stk.attribs, mark);
        let exprs = if self.accept(Token::Assign)? {
            self.exprlist()?
        } else {
            List::EMPTY
        };
        Ok((collective, names, exprs))
    }

    pub(super) fn local_stat(&mut self) -> Result<StatId, SyntaxError> {
        self.advance()?;
        if self.accept(Token::Function)? {
            let line = self.prev_line;
            let name = self.expect_name()?;
            // `local function f` declares `f` in the enclosing function before
            // the body is parsed (PUC `localfunc`'s pre-declare); count it.
            self.new_local()?;
            self.activate_locals()?;
            self.add_local_51(name.sym);
            self.declare([name.sym], VarKind::Local);
            let body = self.func_body(line)?;
            return Ok(self.push_stat(Stat::LocalFunction { name, body }));
        }
        let (collective, names, exprs) = self.attnamelist()?;
        self.activate_locals()?;
        self.declare_attrib_names(names, collective, false);
        for i in names.range() {
            self.add_local_51(self.chunk.attrib_name_lists[i].name.sym);
        }
        Ok(self.push_stat(Stat::Local {
            collective,
            names,
            exprs,
        }))
    }

    pub(super) fn global_stat(&mut self) -> Result<StatId, SyntaxError> {
        self.advance()?;
        if self.accept(Token::Function)? {
            let line = self.prev_line;
            let name = self.expect_name()?;
            self.declare([name.sym], VarKind::Global);
            let body = self.func_body(line)?;
            return Ok(self.push_stat(Stat::GlobalFunction { name, body }));
        }
        // `global [attrib] '*'`
        let leading = self.attrib()?;
        if self.accept(Token::Star)? {
            self.goto_step(|g| {
                g.declare("*", VarKind::Global);
                Ok(())
            })?;
            return Ok(self.push_stat(Stat::GlobalAll { attrib: leading }));
        }
        let mark = self.stk.attribs.len();
        loop {
            let name = self.expect_name()?;
            let attrib = self.attrib()?;
            self.stk.attribs.push_or_abort(AttribName { name, attrib });
            if !self.accept(Token::Comma)? {
                break;
            }
        }
        let names = finish(&mut self.chunk, &mut self.stk.attribs, mark);
        let exprs = if self.accept(Token::Assign)? {
            self.exprlist()?
        } else {
            List::EMPTY
        };
        // the declared names come into scope after their initializers
        self.declare_attrib_names(names, leading, true);
        Ok(self.push_stat(Stat::Global {
            collective: leading,
            names,
            exprs,
        }))
    }

    pub(super) fn expr_stat(&mut self) -> Result<StatId, SyntaxError> {
        let first = self.suffixed_expr()?;
        let is_call = matches!(
            self.chunk.exprs[first.0 as usize],
            Expr::Call { .. } | Expr::MethodCall { .. }
        );
        // 5.1 `exprstat` takes anything that is not a call as the start of
        // an assignment (so a lone `x` wants an '='); 5.2+ look for '=' or
        // ',' first and otherwise demand a call.
        let assign = if self.version <= LuaVersion::Lua51 {
            !is_call
        } else {
            matches!(self.tok.tok, Token::Assign | Token::Comma)
        };
        if !assign {
            if !is_call {
                return Err(self.error("syntax error"));
            }
            return Ok(self.push_stat(Stat::Call(first)));
        }
        // PUC `assignment`/`restassign` check each target as soon as it is
        // parsed, so the near-token is the one following that target.
        let mark = self.stk.exprs.len();
        self.stk.exprs.push_or_abort(first);
        let mut entered = 0;
        loop {
            let last = *self.stk.exprs.last().expect("one target");
            if !matches!(
                self.chunk.exprs[last.0 as usize],
                Expr::Name(_) | Expr::Index { .. }
            ) {
                return Err(self.error("syntax error"));
            }
            // PUC `restassign` checks each target as it reaches the ',' or
            // '=' after it
            if let Expr::Name(n) = self.chunk.exprs[last.0 as usize]
                && self
                    .gotos
                    .as_ref()
                    .is_some_and(|g| g.is_const_local(self.lex.names().text(n.sym)))
            {
                let text = self.text(n.sym).to_owned();
                return Err(
                    self.plain_error(format!("attempt to assign to const variable '{text}'"))
                );
            }
            if !self.accept(Token::Comma)? {
                break;
            }
            // PUC recurses once per extra target and bounds that against
            // the C-call budget (errors.lua :650 expects the error for 500
            // targets), after reading the target: 5.1 as a count of
            // "variables in assignment", 5.2/5.3 as C levels, 5.4+ by
            // entering a level that stays entered until the statement ends.
            let nvars = (self.stk.exprs.len() - mark) as u32;
            let t = self.suffixed_expr()?;
            self.stk.exprs.push_or_abort(t);
            match self.version {
                LuaVersion::Lua51 => {
                    let limit = MAX_DEPTH.saturating_sub(self.depth);
                    if nvars > limit {
                        return Err(self.plain_error(format!(
                            "{} has more than {limit} variables in assignment",
                            self.where_()
                        )));
                    }
                }
                LuaVersion::Lua52 | LuaVersion::Lua53 => {
                    if nvars + self.depth > MAX_DEPTH {
                        return Err(self.levels_error());
                    }
                }
                _ => {
                    self.enter()?;
                    entered += 1;
                }
            }
        }
        let targets = finish(&mut self.chunk, &mut self.stk.exprs, mark);
        self.expect(Token::Assign, "=")?;
        let exprs = self.exprlist()?;
        self.depth -= entered;
        Ok(self.push_stat(Stat::Assign { targets, exprs }))
    }
}
