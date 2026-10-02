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
        for (i, &sid) in self.ls(b.stats).iter().enumerate() {
            let ast = self.ast;
            if let Stat::Label(n) = ast.stat(sid) {
                // a trailing label (only labels after it) does not enter the
                // scope of the block's locals (continue-style jumps); in a
                // repeat body the trailing `until` keeps the locals alive.
                let trailing = !until_follows
                    && self.ls(b.stats)[i + 1..]
                        .iter()
                        .all(|&s| matches!(self.ast.stat(s), Stat::Label(_)));
                self.last_line = n.line;
                self.define_label(self.nm(n), n.line, trailing)?;
                continue;
            }
            self.stat(sid)?;
        }
        Ok(())
    }

    pub(super) fn block_scoped(&mut self, b: &Block) -> Result<(), SyntaxError> {
        self.enter_block(false);
        self.stat_block(b)?;
        self.leave_block()
    }

    pub(super) fn stat(&mut self, sid: StatId) -> Result<(), SyntaxError> {
        // attribute this statement's instructions to its own starting line (PUC
        // tracks the current line as each statement begins), so debug line info,
        // activelines, and line hooks are precise even before the first sub-
        // expression sets a finer line.
        let sline = self.ast.stat_line(sid);
        if sline != 0 {
            self.last_line = sline;
        }
        let ast = self.ast;
        match ast.stat(sid) {
            Stat::Do(b) => self.block_scoped(b),
            Stat::Local {
                collective,
                names,
                exprs,
            } => self.local_stat(*collective, self.ls(*names), self.ls(*exprs)),
            Stat::Assign { targets, exprs } => self.assign_stat(self.ls(*targets), self.ls(*exprs)),
            Stat::If { arms, else_body } => self.if_stat(self.ls(*arms), else_body.as_ref()),
            Stat::While { cond, body } => self.while_stat(*cond, body, self.stat_end_line(sid)),
            Stat::Repeat { body, cond } => self.repeat_stat(body, *cond),
            Stat::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                let end = self.stat_end_line(sid);
                self.numeric_for(self.nm(var), var.line, (*start, *limit, *step), body, end)
            }
            Stat::GenericFor {
                vars,
                exprs,
                body,
                expr_line,
            } => self.generic_for(
                self.ls(*vars),
                self.ls(*exprs),
                body,
                *expr_line,
                self.stat_end_line(sid),
            ),
            Stat::Break { line } => {
                self.last_line = *line;
                let Some(loop_floor) = self
                    .lr()
                    .blocks
                    .iter()
                    .rev()
                    .find(|b| b.is_loop)
                    .map(|b| b.reg_floor)
                else {
                    return Err(self.err(*line, "break outside a loop"));
                };
                // 5.4 jumps to the loop's end and closes there (PUC's
                // "break" label); the others close on the spot
                if self.version != LuaVersion::Lua54 {
                    self.emit(Inst::iabc(Op::Close, loop_floor, 0, 0, false));
                }
                let jmp = self.emit_jump();
                let level = self.lr().locals.len();
                let lp = self
                    .l()
                    .blocks
                    .iter_mut()
                    .rev()
                    .find(|b| b.is_loop)
                    .expect("loop block");
                lp.breaks.push(jmp);
                lp.break_levels.push(level);
                Ok(())
            }
            Stat::Return { exprs, line } => {
                self.last_line = *line;
                self.return_stat(self.ls(*exprs))
            }
            Stat::Call(e) => {
                let e = *e;
                if let Expr::Call { line, .. } | Expr::MethodCall { line, .. } = self.ast.expr(e) {
                    self.last_line = *line;
                }
                let base = self.lr().freereg;
                let ce = self.call_expr(e)?;
                let Exp::Open { pc, .. } = ce else {
                    unreachable!()
                };
                self.patch_wanted(pc, 1); // statement call: zero results
                self.set_freereg(base);
                Ok(())
            }
            Stat::Function { name, body } => self.function_stat(name, body),
            Stat::LocalFunction { name, body } => {
                self.last_line = name.line;
                let reg = self.reserve(1)?;
                // declared before the body: the function can call itself
                self.declare_local(self.nm(name), reg, false)?;
                let f = self.function_exp(body, false)?;
                self.exp_to_reg(f, reg)?;
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
                    .push((text.into(), false));
                self.declare_global_marker(Some(text));
                let saved = self.lr().freereg;
                let f = self.function_exp(body, false)?;
                let r = self.exp_to_anyreg(f)?;
                // `global function f` is a defining write: f must not already
                // exist in the environment (runtime "already defined" check).
                // Pin the redef-check and assignment emits to the name's source
                // line, not the `end` line that `function_exp` just consumed —
                // a chunk with `_ENV = 1` then `global function foo()` should
                // raise on the name's line (errors.lua :521).
                let saved_force = self.force_line.replace(name.line);
                let res = (|| -> Result<(), SyntaxError> {
                    self.emit_global_redef_check(self.nm(name))?;
                    self.assign_global(self.nm(name), r)
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
            Stat::Goto(n) => self.goto_stat(self.nm(n), n.line),
            Stat::Label(_) => unreachable!("labels handled in stat_block"),
        }
    }

    /// 5.5 `global [attrib] name {, name} [= explist]`.
    pub(super) fn global_decl_stat(
        &mut self,
        collective: Option<ast::Attrib>,
        names: &'a [AttribName],
        exprs: &[ExprId],
    ) -> Result<(), SyntaxError> {
        // attribute validation happens before any evaluation
        for an in names {
            let attrib = an.attrib.or(collective);
            if attrib == Some(ast::Attrib::Close) {
                return Err(self.err(an.name.line, "global variables cannot be to-be-closed"));
            }
        }
        let declare = |c: &mut Self| {
            let text = &c.ast.names;
            for an in names {
                let ro = an.attrib.or(collective) == Some(ast::Attrib::Const);
                c.l()
                    .blocks
                    .last_mut()
                    .expect("no block")
                    .gdecls
                    .push((Box::<str>::from(text.text(an.name.sym)), ro));
                c.declare_global_marker(Some(text.text(an.name.sym)));
            }
        };
        if exprs.is_empty() {
            declare(self);
            return Ok(());
        }
        // With an initializer the globals enter scope only AFTER the RHS is
        // evaluated (PUC bumps `nactvar` after the explist), so `global a = a`
        // reads the enclosing `a`, not the global being defined.
        let saved = self.lr().freereg;
        let base = self.explist_adjust(exprs, names.len() as u32)?;
        declare(self);
        // defining write: each target must not already exist (OP_ERRNNIL).
        for (i, an) in names.iter().enumerate() {
            self.emit_global_redef_check(self.nm(&an.name))?;
            self.assign_global(self.nm(&an.name), base + i as u32)?;
        }
        self.set_freereg(saved);
        Ok(())
    }

    pub(super) fn function_stat(
        &mut self,
        name: &FuncName,
        body: &'a FuncBody,
    ) -> Result<(), SyntaxError> {
        self.last_line = name.base.line;
        let is_method = name.method.is_some();
        let saved = self.lr().freereg;
        let f = self.function_exp(body, is_method)?;
        let freg = self.exp_to_anyreg(f)?;
        if name.path.is_empty() && name.method.is_none() {
            self.assign_name(self.nm(&name.base), name.base.line, freg)?;
            self.set_freereg(saved);
            return Ok(());
        }
        // function a.b.c:m — walk to the holder, set the final field.
        // PUC attributes every GETFIELD/SETFIELD on the dotted name to the
        // line of the function statement's name (its `function` keyword),
        // not to the `end` token. Mirror that by pinning `force_line` for
        // the whole holder walk + final store so a `nil` base raises an
        // error on the right source line (errors.lua :430).
        let saved_force = self.force_line.replace(name.base.line);
        let res = (|| -> Result<(), SyntaxError> {
            let be = self.name_expr(self.nm(&name.base))?;
            let mut holder = self.exp_to_anyreg(be)?;
            let mut fields: Vec<&str> = self.ls(name.path).iter().map(|n| self.nm(n)).collect();
            if let Some(m) = &name.method {
                fields.push(self.nm(m));
            }
            for f_name in &fields[..fields.len() - 1] {
                let c = self.str_const(f_name.as_bytes());
                if c <= 0xFF {
                    let pc = self.emit(Inst::iabc(Op::GetField, 0, holder, c, true));
                    let dst = self.reserve(1)?;
                    self.patch_dest(pc, dst);
                    holder = dst;
                } else {
                    let kr = self.reserve(1)?;
                    self.load_const(kr, c);
                    let pc = self.emit(Inst::iabc(Op::GetTable, 0, holder, kr, false));
                    self.patch_dest(pc, kr); // reuse the key register
                    holder = kr;
                }
            }
            let last = &fields[fields.len() - 1];
            let c = self.str_const(last.as_bytes());
            if c <= 0xFF {
                self.emit(Inst::iabc(Op::SetField, holder, c, freg, true));
            } else {
                let kr = self.reserve(1)?;
                self.load_const(kr, c);
                self.emit(Inst::iabc(Op::SetTable, holder, kr, freg, false));
            }
            Ok(())
        })();
        self.force_line = saved_force;
        res?;
        self.set_freereg(saved);
        Ok(())
    }

    pub(super) fn local_stat(
        &mut self,
        collective: Option<ast::Attrib>,
        names: &'a [AttribName],
        exprs: &[ExprId],
    ) -> Result<(), SyntaxError> {
        let n = names.len() as u32;
        // attribute the initialiser instructions to the statement's own line (PUC
        // tracks the current line as each statement starts) so debug line info,
        // activelines, and line hooks see `local x = ...` on its real line.
        if let Some(first) = names.first() {
            self.last_line = first.name.line;
        }
        // PUC `localstat`: with as many values as names, a last name that
        // is <const> and whose value is a compile-time constant is not a
        // variable (5.4+)
        let last_const =
            names.last().and_then(|an| an.attrib.or(collective)) == Some(ast::Attrib::Const);
        let ct = if self.version >= LuaVersion::Lua54 && last_const && exprs.len() == n as usize {
            let ast = self.ast;
            ct_value(ast, exprs[exprs.len() - 1], &mut |name| {
                self.ct_const_named(self.nm(name))
            })
        } else {
            None
        };
        let all_names = names;
        let (names, vals) = match ct {
            Some(_) => (&names[..names.len() - 1], &exprs[..exprs.len() - 1]),
            None => (names, exprs),
        };
        let n = names.len() as u32;
        let base = self.explist_adjust(vals, n)?;
        let mut tbc: Option<u32> = None;
        for (i, an) in names.iter().enumerate() {
            let reg = base + i as u32;
            let attrib = an.attrib.or(collective);
            let read_only = attrib.is_some(); // const and close are both read-only
            if attrib == Some(ast::Attrib::Close) {
                if tbc.is_some() {
                    return Err(self.err(
                        an.name.line,
                        "multiple to-be-closed variables in local list",
                    ));
                }
                tbc = Some(reg);
            }
            self.declare_local(self.nm(&an.name), reg, read_only)?;
        }
        if let (Some(v), Some(last)) = (ct, all_names.last()) {
            self.declare_ct_const(self.nm(&last.name), v);
        }
        if let Some(reg) = tbc {
            self.emit(Inst::iabc(Op::Tbc, reg, 0, 0, false));
            let b = self.l().blocks.last_mut().expect("no block");
            b.has_tbc = true;
            b.tbc_scope = true;
        }
        self.set_freereg(base + n);
        Ok(())
    }

    /// Evaluate an expression list into exactly `want` consecutive registers
    /// starting at the current freereg (nil-padded / truncated; an open last
    /// expression is patched to produce the balance). Returns the base.
    pub(super) fn explist_adjust(
        &mut self,
        exprs: &[ExprId],
        want: u32,
    ) -> Result<u32, SyntaxError> {
        let base = self.lr().freereg;
        // PUC `checkstack`: an open call (`f()`) on the last RHS slot is patched
        // to deliver up to `want` results, bypassing the per-expr `reserve`'s
        // bounds check. errors.lua :721's `local a,a,…(500),a = f()` would
        // otherwise slip past the register cap — guard the target window here.
        if base.saturating_add(want) > max_regs(self.version) {
            return Err(self.regs_error(self.last_line));
        }
        if exprs.is_empty() {
            if want > 0 {
                self.reserve(want)?;
                // PUC 5.1 `luaK_nil`: at function start (pc==0) a LoadNil whose
                // first register is at-or-above nactvar is skipped — locals
                // come in already nil at frame entry, so the op is a no-op.
                // 5.2+ retired the optimization (the LoadNil shows up in the
                // line table either way), so the gate is 5.1-only. db.lua's
                // line-trace tests rely on the suppression — expected line
                // events skip the `local a` line of a chunk that opens with
                // an uninitialized declaration.
                let skip = self.version == LuaVersion::Lua51 && self.lr().code.is_empty();
                if !skip {
                    self.emit(Inst::iabc(Op::LoadNil, base, want - 1, 0, false));
                }
            }
            return Ok(base);
        }
        let n = exprs.len() as u32;
        for (i, &eid) in exprs.iter().enumerate() {
            let dst = base + i as u32;
            if dst >= max_regs(self.version) {
                return Err(self.regs_error(self.last_line));
            }
            self.set_freereg(dst);
            let e = self.expr(eid)?;
            let last = i as u32 == n - 1;
            if last && let Exp::Open { pc, base: ob } = e {
                debug_assert_eq!(ob, dst);
                let missing = (want + 1).saturating_sub(n); // results the open expr must provide
                self.patch_wanted(pc, missing + 1);
                self.set_freereg(base + want.max(n - 1));
                return Ok(base);
            }
            self.set_freereg(dst);
            let got = self.exp_to_nextreg(e)?;
            debug_assert_eq!(got, dst);
        }
        if n < want {
            let first = self.reserve(want - n)?;
            self.emit(Inst::iabc(Op::LoadNil, first, want - n - 1, 0, false));
        }
        self.set_freereg(base + want);
        Ok(base)
    }
}
