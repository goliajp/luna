//! Assignment statements.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn assign_stat(
        &mut self,
        targets: &[ExprId],
        exprs: &[ExprId],
    ) -> Result<(), SyntaxError> {
        let ast = self.ast;
        let saved = self.lr().freereg;
        let want = targets.len() as u32;
        // PUC parses every LHS target (left-to-right) before the RHS explist, so
        // a name first seen on the left captures its upvalue index ahead of one
        // first seen on the right (`a = 10 + b` → a is upvalue 1, b is 2). Pre-
        // resolve the target names in that order; resolution is idempotent and
        // only affects upvalue ordering, not register use or evaluation order.
        for &t in targets {
            self.preresolve_target_upvals(t)?;
        }
        // PUC 5.5 manual §3.3.3: "Lua first evaluates all values from the
        // right-hand side and all index expressions and references on the
        // left-hand side, and only then makes the assignments." Snapshot every
        // Index LHS's obj and key into fresh registers *before* the RHS, so a
        // later store cannot see a local mutated by an earlier store
        // (e.g. attrib.lua `i, a[i], a, j, a[j], a[i+j] = j, i, i, b, j, i`).
        // PUC's `check_conflict` only snapshots locals that actually clash;
        // we copy unconditionally — costs one extra MOVE per Index LHS, much
        // simpler than tracking pairwise conflicts and never wrong.
        let mut plans: SmallList<LhsPlan, 4> = SmallList::new(self.heap.mem());
        for &t in targets {
            match self.ast.expr(t) {
                Expr::Name(_) => plans.push(LhsPlan::Name(t)),
                Expr::Index { obj, key } => {
                    let (obj, key) = (*obj, *key);
                    let oe = self.expr(obj)?;
                    // When the gate certifies the obj is a non-captured
                    // bare-Name local AND the single RHS contains no
                    // UserOrUnknown call, the unconditional snapshot Move
                    // is provably redundant — reuse the local's register
                    // directly. Otherwise fall back to the snapshot.
                    let o_pinned = if self.assign_stat_can_skip_obj_snapshot(targets, exprs)
                        && matches!(oe, Exp::Reg(_))
                    {
                        match oe {
                            Exp::Reg(r) => r,
                            _ => unreachable!(),
                        }
                    } else {
                        self.exp_to_nextreg(oe)?
                    };
                    // Capture the key the same way `assign_to` does: a small
                    // string or int constant rides inline in OP_SetField /
                    // OP_SetI (so it never depends on a register that could
                    // be mutated by an intervening store), everything else
                    // gets pinned to a fresh register too.
                    let key_kind = match ast.expr(key) {
                        Expr::Str(s) if self.sb(*s).len() <= 255 => {
                            let c = self.sym_const(*s);
                            if c <= 0xFF {
                                SetKey::Field(c)
                            } else {
                                let kr = self.reserve(1)?;
                                self.load_const(kr, c);
                                SetKey::Reg(kr)
                            }
                        }
                        Expr::Int(i) if (0..=255).contains(i) => SetKey::Int(*i as u32),
                        _ => {
                            let ke = self.expr(key)?;
                            let kr = match ke {
                                Exp::Reg(r)
                                    if self.assign_stat_can_skip_key_snapshot(targets, exprs) =>
                                {
                                    r
                                }
                                ke => self.exp_to_nextreg(ke)?,
                            };
                            SetKey::Reg(kr)
                        }
                    };
                    plans.push(LhsPlan::Indexed {
                        obj: o_pinned,
                        key: key_kind,
                    });
                }
                _ => unreachable!("parser validates assignment targets"),
            }
        }
        let base = self.explist_adjust(exprs, want)?;
        // When explist_adjust ended with a trivial `Move base, src`
        // materialization of a local-register read AND we have a single
        // store, the store can take `src` directly and the Move is dead.
        // Only catches `Exp::Reg(r)` RHS — Reloc RHS is already handled by
        // the Reloc-landing peephole inside assign_name, literal/Open RHS
        // never emit a tail Move. The pop is guarded by `no_jump_lands_here`:
        // a jump landing at the Move itself is fine (the store popped into its
        // pc reads `src` on every path), one landing after it is not.
        let alt_vreg = if targets.len() == 1 && exprs.len() == 1 && self.no_jump_lands_here() {
            let last_pc = self.here() - 1;
            let last = self.lr().code[last_pc];
            if last.op() == Op::Move && last.a() == base {
                let src = last.b();
                self.l().code.pop();
                self.l().lines.pop();
                Some(src)
            } else {
                None
            }
        } else {
            None
        };
        // PUC `restassign` stores on the way back out of its recursion: the
        // last target first. The order is visible through `__newindex` and
        // when a target repeats (`a, a = 1, 2` leaves 1).
        for i in (0..plans.len()).rev() {
            let vreg = alt_vreg.unwrap_or(base + i as u32);
            match plans.get(i) {
                LhsPlan::Name(t) => self.assign_to(t, vreg)?,
                LhsPlan::Indexed { obj, key } => match key {
                    SetKey::Field(c) => {
                        self.emit(Inst::iabc(Op::SetField, obj, c, vreg, true));
                    }
                    SetKey::Int(c) => {
                        self.emit(Inst::iabc(Op::SetI, obj, c, vreg, false));
                    }
                    SetKey::Reg(k) => {
                        self.emit(Inst::iabc(Op::SetTable, obj, k, vreg, false));
                    }
                },
            }
        }
        self.set_freereg(saved);
        Ok(())
    }

    /// Allocate upvalue indices for the names referenced by an assignment target,
    /// in source order, so they precede the RHS's (PUC restassign ordering). Only
    /// the lvalue prefix (`Name`, and the object/key of an `Index`) is walked.
    pub(super) fn preresolve_target_upvals(&mut self, id: ExprId) -> Result<(), SyntaxError> {
        if crate::native_stack::is_low(crate::native_stack::RESERVE) {
            return Err(self.too_deep());
        }
        let ast = self.ast;
        // the objects of a chain of indices come first, innermost first
        let mut keys: Vec<ExprId> = Vec::new();
        let mut cur = id;
        while let Expr::Index { obj, key } = *ast.expr(cur) {
            keys.push(key);
            cur = obj;
        }
        if let Expr::Name(n) = ast.expr(cur) {
            self.resolve_name(self.nm(n))?;
        }
        for key in keys.into_iter().rev() {
            self.preresolve_target_upvals(key)?;
        }
        Ok(())
    }

    pub(super) fn assign_name(
        &mut self,
        text: &str,
        line: u32,
        vreg: u32,
    ) -> Result<(), SyntaxError> {
        // PUC `restassign` emits the store with `ls->lastline` (the last token
        // of the rhs), not the line of the lhs name, so a multi-line
        // `a = b[1] \n + \n b[1]` attributes SETTABUP/SETUPVAL to the rhs's
        // end line (db.lua :193 line-trace family). Errors use the explicit
        // `line` param verbatim so a read-only-assign diagnostic still
        // points at the name.
        match self.resolve_name(text)? {
            VarKind::Const(_) => Err(self.err(
                line,
                format!("attempt to assign to const variable '{text}'"),
            )),
            VarKind::Local(reg) => {
                if let Some(name) = self.local_is_read_only(reg) {
                    let name = name.to_string();
                    return Err(self.err(
                        line,
                        format!("attempt to assign to const variable '{name}'"),
                    ));
                }
                if reg != vreg {
                    // Reloc-landing peephole: when the just-emitted
                    // instruction is a retargetable producer (Add / GetField
                    // / Unm / Len / etc.) that wrote into `vreg` and no jump
                    // lands right after it, retarget its A field to
                    // `reg` and skip the Move. Mirrors PUC `discharge2reg`'s
                    // A-field rewrite at lcode.c:luaK_dischargevars / setoneret.
                    if let Some(prev_pc) = self.assign_name_can_retarget_reloc(vreg) {
                        self.patch_dest(prev_pc, reg);
                    } else {
                        self.emit(Inst::iabc(Op::Move, reg, vreg, 0, false));
                    }
                }
                Ok(())
            }
            VarKind::Upval(u) => {
                if self.lr().upvals[u as usize].read_only {
                    return Err(self.err(
                        line,
                        format!("attempt to assign to const variable '{text}'"),
                    ));
                }
                self.emit(Inst::iabc(Op::SetUpval, vreg, u, 0, false));
                Ok(())
            }
            VarKind::Global { .. } => {
                let VarKind::Global { read_only } = self.resolve_global_kind(text, line)? else {
                    unreachable!()
                };
                if read_only {
                    return Err(self.err(
                        line,
                        format!("attempt to assign to const variable '{text}'"),
                    ));
                }
                self.assign_global(text, vreg)
            }
        }
    }

    pub(super) fn assign_global(&mut self, text: &str, vreg: u32) -> Result<(), SyntaxError> {
        if self.env_is_global() {
            return Err(self.err(
                self.last_line,
                format!("_ENV is global when accessing variable '{text}'"),
            ));
        }
        let c = self.str_const(text.as_bytes());
        match self.resolve_env()? {
            VarKind::Upval(u) if c <= 0xFF => {
                self.emit(Inst::iabc(Op::SetTabUp, u, c, vreg, true));
                Ok(())
            }
            VarKind::Local(r) if c <= 0xFF => {
                self.emit(Inst::iabc(Op::SetField, r, c, vreg, true));
                Ok(())
            }
            env => {
                let saved = self.lr().freereg;
                let er = self.reserve(2)?;
                match env {
                    VarKind::Upval(u) => {
                        self.emit(Inst::iabc(Op::GetUpval, er, u, 0, false));
                    }
                    VarKind::Local(r) => {
                        self.emit(Inst::iabc(Op::Move, er, r, 0, false));
                    }
                    VarKind::Global { .. } | VarKind::Const(_) => {
                        unreachable!("resolve_env gives a register or an upvalue")
                    }
                }
                self.load_const(er + 1, c);
                self.emit(Inst::iabc(Op::SetTable, er, er + 1, vreg, false));
                self.set_freereg(saved);
                Ok(())
            }
        }
    }

    pub(super) fn assign_to(&mut self, target: ExprId, vreg: u32) -> Result<(), SyntaxError> {
        let ast = self.ast;
        match ast.expr(target) {
            Expr::Name(n) => self.assign_name(self.nm(n), n.line, vreg),
            Expr::Index { obj, key } => {
                let (obj, key) = (*obj, *key);
                let saved = self.lr().freereg;
                let oe = self.expr(obj)?;
                let o = self.exp_to_anyreg(oe)?;
                match ast.expr(key) {
                    Expr::Str(s) if self.sb(*s).len() <= 255 => {
                        let c = self.sym_const(*s);
                        if c <= 0xFF {
                            self.emit(Inst::iabc(Op::SetField, o, c, vreg, true));
                        } else {
                            let kr = self.reserve(1)?;
                            self.load_const(kr, c);
                            self.emit(Inst::iabc(Op::SetTable, o, kr, vreg, false));
                        }
                    }
                    Expr::Int(i) if (0..=255).contains(i) => {
                        let c = *i as u32;
                        self.emit(Inst::iabc(Op::SetI, o, c, vreg, false));
                    }
                    _ => {
                        let ke = self.expr(key)?;
                        let k = self.exp_to_anyreg(ke)?;
                        self.emit(Inst::iabc(Op::SetTable, o, k, vreg, false));
                    }
                }
                self.set_freereg(saved);
                Ok(())
            }
            _ => unreachable!("parser validates assignment targets"),
        }
    }
}
