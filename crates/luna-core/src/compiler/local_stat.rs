//! `local` declarations.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn local_stat(
        &mut self,
        sid: StatId,
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
        let eligible = self.version >= LuaVersion::Lua54 && last_const && exprs.len() == n as usize;
        let ast = self.ast;
        let last_expr = exprs[exprs.len().saturating_sub(1)..].first().copied();
        let ct = match last_expr {
            Some(e) if eligible => ct_value(ast, e, &mut |name| self.ct_const_named(self.nm(name))),
            _ => None,
        };
        // PUC also takes `X and nil or K` as the constant K, once it has
        // compiled X: the initialiser still runs for X, x is not a variable
        let run_last = match last_expr {
            Some(e) if eligible && ct.is_none() => {
                ct_operand(ast, e, &mut |name| self.ct_const_named(self.nm(name)))
            }
            _ => None,
        };
        let ct = ct.or(run_last.clone());
        let all_names = names;
        let (names, vals) = match ct {
            Some(_) => (&names[..names.len() - 1], &exprs[..exprs.len() - 1]),
            None => (names, exprs),
        };
        let n = names.len() as u32;
        // with no values the names are adjusted where the statement ends
        self.log_local_names(sid, collective, all_names)?;
        self.at(TokRef::StatAfter(sid, 0));
        let base = self.explist_adjust(vals, n)?;
        if let (Some(_), Some(e)) = (&run_last, last_expr) {
            self.set_freereg(base + n);
            // PUC keeps the code of X and makes no load of the constant
            // (`luaK_exp2const` takes the value once its jumps are patched)
            let v = self.expr(e)?;
            if matches!(v, Exp::Jumps(_) | Exp::Jmp(_)) {
                self.exp_to_nextreg(v)?;
            }
            self.set_freereg(base + n);
        }
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
}
