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
        self.mark();
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
            self.mark();
            self.stk.arms.push_or_abort(IfArm {
                cond,
                then_line,
                body,
            });
        }
        let arms = finish(self.chunk, &mut self.stk.arms, mark);
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
        self.mark();
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
                // the loop's hidden control locals count against the
                // local limit from the header on, declared before the `=`
                // (PUC `fornum`): three up to 5.4, two from 5.5
                let hidden = if self.version >= LuaVersion::Lua55 {
                    2
                } else {
                    3
                };
                self.new_locals(hidden + 1)?;
                self.advance()?;
                let start = self.expr()?;
                self.expect(Token::Comma, ",")?;
                let limit = self.expr()?;
                let step = if self.accept(Token::Comma)? {
                    Some(self.expr()?)
                } else {
                    None
                };
                // 5.5 opens the hidden ones on the `do`
                self.activate_hidden_55(hidden);
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
                // PUC `forlist`: three hidden locals (generator, state,
                // control), four in 5.4 (plus the closing value), three
                // again in 5.5, declared with the first name, each other
                // name once it is read
                let hidden = if self.version == LuaVersion::Lua54 {
                    4
                } else {
                    3
                };
                self.new_locals(hidden + 1)?;
                let mark = self.stk.names.len();
                self.stk.names.push_or_abort(first);
                while self.accept(Token::Comma)? {
                    let n = self.expect_name()?;
                    self.new_local()?;
                    self.stk.names.push_or_abort(n);
                }
                let vars = finish(self.chunk, &mut self.stk.names, mark);
                self.expect(Token::In, "in")?;
                let expr_line = self.tok.line;
                let exprs = self.exprlist()?;
                self.activate_hidden_55(hidden);
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
            self.check_target(last)?;
            if !self.accept(Token::Comma)? {
                break;
            }
            // bounded after reading the target
            let nvars = (self.stk.exprs.len() - mark) as u32;
            let t = self.suffixed_expr()?;
            self.stk.exprs.push_or_abort(t);
            self.count_target(nvars, &mut entered)?;
        }
        let targets = finish(self.chunk, &mut self.stk.exprs, mark);
        self.expect(Token::Assign, "=")?;
        let exprs = self.exprlist()?;
        self.depth -= entered;
        Ok(self.push_stat(Stat::Assign { targets, exprs }))
    }

    /// An assignment's target `last`, checked as PUC's `restassign` checks
    /// it when it reaches the ',' or '=' after it.
    fn check_target(&mut self, last: ExprId) -> Result<(), SyntaxError> {
        // a completion (lenient) takes any target: what PUC made of it
        // before the error is still counted
        if !matches!(
            self.chunk.exprs[last.0 as usize],
            Expr::Name(_) | Expr::Index { .. }
        ) && !self.stk.lenient
        {
            return Err(self.error("syntax error"));
        }
        // PUC `restassign` checks each target as it reaches the ',' or
        // '=' after it
        if let Expr::Name(n) = self.chunk.exprs[last.0 as usize]
            && !self.stk.lenient
            && self
                .gotos
                .as_ref()
                .is_some_and(|g| g.is_const_local(self.lex.names().text(n.sym)))
        {
            let text = self.text(n.sym).to_owned();
            return Err(self.plain_error(format!("attempt to assign to const variable '{text}'")));
        }
        Ok(())
    }

    /// Another target after `nvars`, bounded as PUC bounds its recursion
    /// against the C-call budget (errors.lua :650 expects the error for 500
    /// targets): 5.1 as a count of "variables in assignment", 5.2/5.3 as C
    /// levels, 5.4+ by entering a level that stays entered until the
    /// statement ends (`entered`).
    fn count_target(&mut self, nvars: u32, entered: &mut u32) -> Result<(), SyntaxError> {
        // a completion (lenient) takes any number: PUC's parse up to
        // the error is what is counted
        match self.version {
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53 if self.stk.lenient => {}
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
                *entered += 1;
            }
        }
        Ok(())
    }
}
