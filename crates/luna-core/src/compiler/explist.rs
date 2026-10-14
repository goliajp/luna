//! Expression lists adjusted to a number of values, and 5.5 `global`
//! declarations.

use super::*;

impl<'a> Compiler<'a> {
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
                    .push_or_abort((text.text(an.name.sym), ro));
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
        let mut lvs = self.lvs.take();
        for an in names {
            let lv = self.global_lv(self.nm(&an.name))?;
            lvs.push_or_abort(lv);
        }
        let base = self.explist_adjust(exprs, names.len() as u32)?;
        declare(self);
        // defining write: each target must not already exist (OP_ERRNNIL);
        // PUC checks and stores the last one first, the check reading into
        // the register above the values still to store
        for (i, an) in names.iter().enumerate().rev() {
            self.set_freereg(base + i as u32 + 1);
            self.emit_global_redef_check(self.nm(&an.name))?;
            self.store(lvs[i], Exp::Reg(base + i as u32))?;
        }
        lvs.clear();
        self.lvs = lvs;
        self.set_freereg(saved);
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
                    self.load_nil(base, want);
                }
            }
            return Ok(base);
        }
        let n = exprs.len() as u32;
        for (i, &eid) in exprs.iter().enumerate() {
            let dst = base + i as u32;
            self.set_freereg(dst);
            let e = self.expr(eid)?;
            let last = i as u32 == n - 1;
            // a value goes to its register once the `,` after it is read,
            // the last is adjusted once the list ends
            self.at(TokRef::After(eid, i32::from(!last)));
            if last && let Exp::Open { pc, base: ob } = e {
                debug_assert_eq!(ob, dst);
                // PUC `luaK_setreturns` checks the registers the results
                // take (an open call on the last value bypasses the
                // per-value `reserve`)
                if base.saturating_add(want) > max_regs(self.version) {
                    return Err(self.regs_error());
                }
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
            self.load_nil(first, want - n);
        }
        self.set_freereg(base + want);
        Ok(base)
    }
}
