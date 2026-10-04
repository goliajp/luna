//! Blocks, the statement dispatcher, and block-ending statements.

use super::*;

impl<'s> Parser<'s> {
    pub(super) fn block_follow(&self) -> bool {
        matches!(
            self.tok.tok,
            Token::Eof | Token::End | Token::Else | Token::Elseif | Token::Until
        )
    }

    pub(super) fn block(&mut self) -> Result<Block, SyntaxError> {
        self.enter()?;
        // PUC `leaveblock` restores `nactvar` to the count at block entry, so
        // a block's locals fall out of scope when it ends. Snapshot the count
        // here so the limit check tracks ACTIVE locals (locals.lua opens many
        // short blocks; without this the cap fires spuriously).
        let local_snapshot = self.func_local_count.last().expect("func ctx").0;
        let locals_51_snap = self.snap_locals_51();
        self.goto_step(|g| {
            g.enter_block(false);
            Ok(())
        })?;
        let mark = self.stk.stats.len();
        loop {
            // labels wait for the no-op statements that follow them
            if self.gotos.as_ref().is_some_and(GotoCheck::has_open_labels)
                && !matches!(self.tok.tok, Token::Semi | Token::DColon)
            {
                let last = matches!(
                    self.tok.tok,
                    Token::Else | Token::Elseif | Token::End | Token::Eof
                );
                self.goto_step(|g| g.finish_labels(last))?;
            }
            if self.block_follow() {
                break;
            }
            if self.tok.tok == Token::Return {
                let s = self.return_stat()?;
                self.stk.stats.push(s);
                break;
            }
            if self.tok.tok == Token::Break && self.version.break_is_last_statement() {
                let line = self.tok.line;
                self.break_stat()?;
                let s = self.push_stat(Stat::Break { line });
                self.stk.stats.push(s);
                self.accept(Token::Semi)?;
                break;
            }
            if let Some(s) = self.statement()? {
                self.stk.stats.push(s);
            }
            if !self.version.has_empty_statement() {
                // 5.1: ';' is a separator after a statement, not a statement
                self.accept(Token::Semi)?;
            }
        }
        self.goto_step(GotoCheck::leave_block)?;
        self.leave();
        self.func_local_count.last_mut().expect("func ctx").0 = local_snapshot;
        self.restore_locals_51(locals_51_snap);
        Ok(Block {
            stats: finish(&mut self.chunk, &mut self.stk.stats, mark),
        })
    }

    pub(super) fn return_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let exprs = if self.block_follow() || self.tok.tok == Token::Semi {
            List::EMPTY
        } else {
            self.exprlist()?
        };
        self.accept(Token::Semi)?;
        Ok(self.push_stat(Stat::Return { exprs, line }))
    }

    pub(super) fn statement(&mut self) -> Result<Option<StatId>, SyntaxError> {
        // PUC's `statement` does not bump `nCcalls` itself — the surrounding
        // `block` does, and nested forms (do/while/if/function/...) each
        // recurse through `block` again. Counting both would double the cost
        // per `do … end` nesting; errors.lua's `testrep("do ", "", " end")`
        // expects 190 levels to compile and 201 to fail at the same wall as
        // the other shapes.
        let start_line = self.tok.line;
        // 5.5 `global` is a contextual keyword: a declaration only when it
        // leads a statement and the next token starts one (name / '*' /
        // function / attribute '<'). Otherwise it is an ordinary identifier
        // (e.g. `global = 1`, `global()`, `return global`).
        if self.version.has_global_decl()
            && matches!(&self.tok.tok, Token::Name(_))
            && self.text(self.tok_sym) == "global"
            && matches!(
                self.peek()?,
                Token::Name(_) | Token::Star | Token::Function | Token::Lt
            )
        {
            let stat = self.global_stat()?;
            self.set_stat_line(stat, start_line);
            return Ok(Some(stat));
        }
        let stat = match self.tok.tok {
            Token::Semi => {
                if !self.version.has_empty_statement() {
                    return Err(self.error("unexpected symbol"));
                }
                self.advance()?;
                None
            }
            Token::If => Some(self.if_stat()?),
            Token::While => Some(self.while_stat()?),
            Token::Do => {
                let line = self.tok.line;
                self.advance()?;
                let body = self.block()?;
                self.expect_match(Token::End, "end", "do", line)?;
                Some(self.push_stat(Stat::Do(body)))
            }
            Token::For => Some(self.for_stat()?),
            Token::Repeat => Some(self.repeat_stat()?),
            Token::Function => Some(self.function_stat()?),
            Token::Local => Some(self.local_stat()?),
            Token::DColon => {
                self.advance()?;
                let name = self.expect_name()?;
                let text = self.text(name.sym).to_owned();
                self.goto_step(|g| g.label_before_close(&text, start_line))?;
                self.expect(Token::DColon, "::")?;
                Some(self.push_stat(Stat::Label(name)))
            }
            Token::Break => {
                let line = self.tok.line;
                self.break_stat()?;
                Some(self.push_stat(Stat::Break { line }))
            }
            Token::Goto => {
                // 5.4 reads the goto's line after skipping the keyword, 5.5
                // takes the statement's
                let mut line = self.lex.line();
                self.advance()?;
                if self.version >= LuaVersion::Lua55 {
                    line = start_line;
                } else if self.version >= LuaVersion::Lua54 {
                    line = self.lex.line();
                }
                let name = self.expect_name()?;
                let text = self.text(name.sym).to_owned();
                self.goto_step(|g| g.goto_stat(&text, line))?;
                Some(self.push_stat(Stat::Goto(name)))
            }
            _ => Some(self.expr_stat()?),
        };
        if let Some(sid) = stat {
            self.set_stat_line(sid, start_line);
        }
        Ok(stat)
    }

    pub(super) fn set_stat_line(&mut self, sid: StatId, line: u32) {
        let idx = sid.0 as usize;
        if self.chunk.stat_lines.len() <= idx {
            self.chunk.stat_lines.resize(idx + 1, 0);
        }
        self.chunk.stat_lines[idx] = line;
    }

    /// Consume `break`, checking it the way the dialect does: 5.1 and 5.5
    /// on the spot (5.1 after skipping the keyword, 5.5 before); 5.2–5.4
    /// treat it as a goto to the loop's end, so a break outside a loop is
    /// an unresolved goto when the function closes.
    pub(super) fn break_stat(&mut self) -> Result<(), SyntaxError> {
        let line = self.lex.line();
        let in_loop = self.funcs.last().expect("func ctx").loops > 0;
        if !in_loop && self.version >= LuaVersion::Lua55 {
            return Err(self.error("break outside loop"));
        }
        self.advance()?;
        if !in_loop && self.version <= LuaVersion::Lua51 {
            return Err(self.error("no loop to break"));
        }
        if self.version >= LuaVersion::Lua55 {
            return Ok(());
        }
        self.goto_step(|g| g.goto_stat("break", line))
    }

    /// A loop body with the loop's own variables (`vars`) in scope: PUC's
    /// loop block, which places the "break" label, around a block for the
    /// declared variables.
    pub(super) fn loop_block(&mut self, vars: List<Name>) -> Result<Block, SyntaxError> {
        self.funcs.last_mut().expect("func ctx").loops += 1;
        self.goto_step(|g| {
            g.enter_block(true);
            g.enter_block(false);
            Ok(())
        })?;
        // 5.5: the control (first) variable of a loop is read-only
        let v55 = self.version >= LuaVersion::Lua55;
        let Parser {
            gotos, lex, chunk, ..
        } = self;
        if let Some(g) = gotos {
            for (i, v) in chunk.list(vars).iter().enumerate() {
                let kind = if i == 0 && v55 {
                    VarKind::Const
                } else {
                    VarKind::Local
                };
                g.declare(lex.names().text(v.sym), kind);
            }
        }
        let body = self.block()?;
        self.goto_step(|g| {
            g.leave_block()?;
            g.leave_block()
        })?;
        self.funcs.last_mut().expect("func ctx").loops -= 1;
        Ok(body)
    }

    /// PUC `close_func` → `leaveblock` of the function's outer block, which
    /// runs after the closing token has been consumed: a goto (or 5.2-5.4
    /// `break`) that found no label is reported there, at the scanner's
    /// line.
    pub(super) fn close_function(&mut self) -> Result<(), SyntaxError> {
        let _ = self.funcs.pop().expect("func ctx");
        self.goto_step(GotoCheck::leave_block)
    }

    /// Run a step of the goto check (dialects that have one), turning its
    /// error into a syntax error without a near-token (PUC `semerror`),
    /// which 5.5 reports at the line of the last token consumed.
    pub(super) fn goto_step(
        &mut self,
        step: impl FnOnce(&mut GotoCheck) -> Result<(), String>,
    ) -> Result<(), SyntaxError> {
        match self.gotos.as_mut().map(step) {
            Some(Err(msg)) if self.version >= LuaVersion::Lua55 => Err(SyntaxError {
                line: self.last_line,
                msg: msg.into_bytes(),
            }),
            Some(Err(msg)) => Err(self.plain_error(msg)),
            _ => Ok(()),
        }
    }
}
