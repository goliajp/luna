//! Statement dispatch and declarations (`local`, `global`, `function`).

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn stat_block(&mut self, b: &Block) -> Result<(), SyntaxError> {
        self.stat_block_inner(b, false)
    }

    /// `until_follows` marks a repeat body: its `until` condition is still in
    /// the scope of the body's locals, so a label at the body's end is NOT
    /// trailing — a goto into it lands in those locals' scope (PUC matches a
    /// label with `block_follow(ls, 0)`, which excludes `until`).
    pub(super) fn stat_block_inner(
        &mut self,
        b: &Block,
        until_follows: bool,
    ) -> Result<(), SyntaxError> {
        self.stat_list(self.ls(b.stats), until_follows)
    }

    /// The statements `stats` of a block (see [`Self::stat_block_inner`]).
    pub(super) fn stat_list(
        &mut self,
        stats: &'a [StatId],
        until_follows: bool,
    ) -> Result<(), SyntaxError> {
        for (i, &sid) in stats.iter().enumerate() {
            let ast = self.ast;
            if let Stat::Label(n) = ast.stat(sid) {
                // a trailing label (only labels after it) does not enter the
                // scope of the block's locals (continue-style jumps); in a
                // repeat body the trailing `until` keeps the locals alive.
                let trailing = !until_follows
                    && stats[i + 1..]
                        .iter()
                        .all(|&s| matches!(self.ast.stat(s), Stat::Label(_)));
                self.last_line = n.line;
                self.at(TokRef::StatAfter(sid, 0));
                self.define_label(self.nm(n), trailing)?;
                continue;
            }
            self.stat(sid)?;
        }
        Ok(())
    }

    /// Block `b` in a scope of its own, left at token `end` (the one that
    /// ends it).
    pub(super) fn block_scoped(&mut self, b: &Block, end: TokRef) -> Result<(), SyntaxError> {
        self.enter_block(false);
        self.stat_block(b)?;
        self.at(end);
        self.leave_block()
    }

    pub(super) fn stat(&mut self, sid: StatId) -> Result<(), SyntaxError> {
        // attribute this statement's instructions to its own starting line (PUC
        // tracks the current line as each statement begins), so debug line info,
        // activelines, and line hooks are precise even before the first sub-
        // expression sets a finer line.
        let sline = self.ast.stat_line(sid);
        // 5.2 / 5.3 emit the jump of a `break` / `goto` before reading the
        // keyword: it takes the line of the token before
        let jumps_first = matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53)
            && matches!(self.ast.stat(sid), Stat::Break { .. } | Stat::Goto(_));
        if sline != 0 && !jumps_first {
            self.last_line = sline;
        }
        self.at(TokRef::StatFirst(sid, 0));
        self.stat_body(sid)?;
        // the statement's last token is read
        if let Some(line) = self.stat_end_line(sid) {
            self.last_line = line;
        }
        self.jump_error()
    }

    fn stat_body(&mut self, sid: StatId) -> Result<(), SyntaxError> {
        let ast = self.ast;
        match ast.stat(sid) {
            Stat::Do(b) => self.block_scoped(b, TokRef::StatAfter(sid, -1)),
            Stat::Local {
                collective,
                names,
                exprs,
            } => self.local_stat(sid, *collective, self.ls(*names), self.ls(*exprs)),
            Stat::Assign { targets, exprs } => self.assign_stat(self.ls(*targets), self.ls(*exprs)),
            Stat::If { arms, else_body } => self.if_stat(sid, self.ls(*arms), else_body.as_ref()),
            Stat::While { cond, body } => {
                self.while_stat(sid, *cond, body, self.stat_end_line(sid))
            }
            Stat::Repeat { body, cond } => self.repeat_stat(body, *cond),
            Stat::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                let end = self.stat_end_line(sid);
                self.numeric_for(sid, var, var.line, (*start, *limit, *step), body, end)
            }
            Stat::GenericFor {
                vars,
                exprs,
                body,
                expr_line,
            } => self.generic_for(
                sid,
                self.ls(*vars),
                self.ls(*exprs),
                body,
                *expr_line,
                self.stat_end_line(sid),
            ),
            Stat::Break { line } => {
                // 5.2 / 5.3 jump before reading `break`, the others after
                let skip = !matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53);
                self.at(TokRef::StatFirst(sid, i32::from(skip)));
                self.break_stat(*line)
            }
            Stat::Return { exprs, line } => {
                self.last_line = *line;
                self.at(TokRef::StatFirst(sid, 1));
                self.return_stat(self.ls(*exprs))
            }
            Stat::Call(e) => {
                let e = *e;
                if let Expr::Call { line, .. } | Expr::MethodCall { line, .. } = self.ast.expr(e) {
                    self.last_line = *line;
                }
                let base = self.lr().freereg;
                let ce = self.expr(e)?;
                let Exp::Open { pc, .. } = ce else {
                    unreachable!()
                };
                self.patch_wanted(pc, 1); // statement call: zero results
                self.set_freereg(base);
                Ok(())
            }
            Stat::Function { name, body } => self.function_stat(sid, name, body),
            Stat::LocalFunction { name, body } => {
                self.last_line = name.line;
                // the closure goes to the variable's register after `end`
                self.at(TokRef::StatAfter(sid, 0));
                let reg = self.reserve(1)?;
                // declared before the body, when the scanner is on its `(`:
                // the function can call itself
                self.at(TokRef::StatMark(sid, 0));
                self.declare_local(self.nm(name), NameSrc::Sym(name.sym), reg, false)?;
                // PUC does not reserve the variable's register before the
                // body: the closure takes the next free one, the variable's
                self.set_freereg(reg);
                let f = self.function_exp(
                    body,
                    false,
                    TokRef::StatMark(sid, 0),
                    TokRef::StatAfter(sid, 0),
                )?;
                self.exp_to_reg(f, reg)?;
                // debug information sees the variable once the closure is in it
                let pc = self.lr().code.len() as u32;
                if let Some(l) = self.l().locals.last_mut() {
                    l.start_pc = pc;
                }
                self.set_freereg(reg + 1);
                Ok(())
            }
            Stat::GlobalFunction { name, body } => {
                // `global function f` declares f, then assigns the closure
                self.last_line = name.line;
                let text = self.nm(name);
                self.l()
                    .blocks
                    .last_mut()
                    .expect("no block")
                    .gdecls
                    .push_or_abort((text, false));
                self.declare_global_marker(Some(text));
                let saved = self.lr().freereg;
                let lv = self.global_lv(text)?;
                let f = self.function_exp(
                    body,
                    false,
                    TokRef::StatMark(sid, 0),
                    TokRef::StatAfter(sid, 0),
                )?;
                // `global function f` is a defining write: f must not already
                // exist in the environment (runtime "already defined" check).
                // Pin the redef-check and assignment emits to the name's source
                // line, not the `end` line that `function_exp` just consumed —
                // a chunk with `_ENV = 1` then `global function foo()` should
                // raise on the name's line (errors.lua :521).
                let saved_force = self.force_line.replace(name.line);
                let res = (|| -> Result<(), SyntaxError> {
                    self.emit_global_redef_check(self.nm(name))?;
                    self.store(lv, f)
                })();
                self.force_line = saved_force;
                res?;
                self.set_freereg(saved);
                Ok(())
            }
            Stat::Global {
                collective,
                names,
                exprs,
            } => self.global_decl_stat(*collective, self.ls(*names), self.ls(*exprs)),
            Stat::GlobalAll { attrib } => {
                let attrib = *attrib;
                if attrib == Some(ast::Attrib::Close) {
                    return Err(self.err(self.last_line, "global variables cannot be to-be-closed"));
                }
                let ro = attrib == Some(ast::Attrib::Const);
                self.l().blocks.last_mut().expect("no block").collective = Some(ro);
                // a `global *` marker participates in goto-scope checks ('*')
                self.declare_global_marker(None);
                Ok(())
            }
            Stat::Goto(n) => {
                // 5.2 / 5.3 jump before reading `goto`, the others after the
                // label's name
                let skip = if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
                    0
                } else {
                    2
                };
                self.at(TokRef::StatFirst(sid, skip));
                self.goto_stat(self.nm(n), n.line, TokRef::StatFirst(sid, 2))
            }
            Stat::Label(_) => unreachable!("labels handled in stat_block"),
        }
    }
}
