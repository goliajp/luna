//! Assignment statements, compiled the way PUC's `restassign` does: every
//! target first (its table and key left where `luaK_indexed` puts them),
//! then the values, then the stores, last target first.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn assign_stat(
        &mut self,
        targets: &[ExprId],
        exprs: &[ExprId],
    ) -> Result<(), SyntaxError> {
        let saved = self.lr().freereg;
        let mut lvs: LVec<Lv> = LVec::new(self.heap.mem());
        for &t in targets {
            let lv = self.target_lv(t)?;
            // a variable assigned after an indexing that reads it: the
            // indexing keeps a copy of its current value
            if !matches!(lv, Lv::Indexed(..)) {
                self.check_conflict(&mut lvs, lv)?;
            }
            lvs.push_or_abort(lv);
        }
        let nvars = lvs.len();
        let base = if exprs.len() == nvars {
            // as many values as targets: the last value goes straight into
            // the last target, the others into registers
            let base = self.lr().freereg;
            for (i, &eid) in exprs[..nvars - 1].iter().enumerate() {
                let dst = base + i as u32;
                self.set_freereg(dst);
                let e = self.expr(eid)?;
                self.set_freereg(dst);
                self.exp_to_nextreg(e)?;
            }
            self.set_freereg(base + nvars as u32 - 1);
            let e = self.expr(exprs[nvars - 1])?;
            self.store(lvs[nvars - 1], e)?;
            lvs.pop();
            base
        } else {
            self.explist_adjust(exprs, nvars as u32)?
        };
        // the order of the stores shows through `__newindex` and when a
        // target repeats (`a, a = 1, 2` leaves 1)
        for (i, &lv) in lvs.iter().enumerate().rev() {
            self.store(lv, Exp::Reg(base + i as u32))?;
        }
        self.set_freereg(saved);
        Ok(())
    }

    /// An assignment target as PUC's `suffixedexp` leaves it, checked for
    /// being read-only.
    fn target_lv(&mut self, t: ExprId) -> Result<Lv, SyntaxError> {
        let ast = self.ast;
        match *ast.expr(t) {
            Expr::Name(ref n) => self.name_lv(self.nm(n), n.line),
            Expr::Index { obj, key } => {
                let oe = self.expr(obj)?;
                let tr = self.index_table(oe)?;
                let ke = self.expr(key)?;
                let (tr, k) = self.indexed(tr, ke)?;
                Ok(Lv::Indexed(tr, k))
            }
            _ => unreachable!("parser validates assignment targets"),
        }
    }

    /// The variable `text` as an assignment target.
    pub(super) fn name_lv(&mut self, text: &str, line: u32) -> Result<Lv, SyntaxError> {
        let read_only = |c: &Self, name: &str| {
            c.err(line, format!("attempt to assign to const variable '{name}'"))
        };
        match self.resolve_name(text)? {
            VarKind::Const(_) => Err(read_only(self, text)),
            VarKind::Local(reg) => {
                if let Some(name) = self.local_is_read_only(reg) {
                    let name = name.to_string();
                    return Err(read_only(self, &name));
                }
                Ok(Lv::Local(reg))
            }
            VarKind::Upval(u) => {
                if self.lr().upvals[u as usize].read_only {
                    return Err(read_only(self, text));
                }
                Ok(Lv::Upval(u))
            }
            VarKind::Global { .. } => {
                let VarKind::Global { read_only: ro } = self.resolve_global_kind(text, line)? else {
                    unreachable!()
                };
                if ro {
                    return Err(read_only(self, text));
                }
                self.global_lv(text)
            }
        }
    }

    /// Global `text` as an assignment target: `_ENV[text]`.
    pub(super) fn global_lv(&mut self, text: &str) -> Result<Lv, SyntaxError> {
        if self.env_is_global() {
            return Err(self.err(
                self.last_line,
                format!("_ENV is global when accessing variable '{text}'"),
            ));
        }
        let t = match self.resolve_env()? {
            VarKind::Upval(u) => TabRef::Up(u),
            VarKind::Local(r) => TabRef::Reg(r),
            VarKind::Global { .. } | VarKind::Const(_) => {
                unreachable!("resolve_env gives a register or an upvalue")
            }
        };
        let c = self.str_const(text.as_bytes());
        if self.version == LuaVersion::Lua51 && c > MAX_C {
            return Ok(Lv::Indexed(t, KeyRef::K(c)));
        }
        let (t, k) = self.indexed(t, Exp::Const(c))?;
        Ok(Lv::Indexed(t, k))
    }
}
