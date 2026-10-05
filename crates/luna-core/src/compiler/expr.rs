//! Expression dispatch, calls, and discharging an `Exp` into registers
//! (names are in `expr_names`).

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn expr(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        let ast = self.ast;
        match ast.expr(id) {
            Expr::Nil => Ok(Exp::Nil),
            Expr::True => Ok(Exp::True),
            Expr::False => Ok(Exp::False),
            Expr::Int(i) => Ok(Exp::Int(*i)),
            Expr::Float(f) => Ok(Exp::Float(*f)),
            Expr::Str(s) => Ok(Exp::Const(self.sym_const(*s))),
            Expr::Name(n) => {
                self.last_line = n.line;
                self.name_expr(self.nm(n))
            }
            Expr::Paren(inner) => {
                // parentheses truncate multiple results to exactly one
                let e = self.expr(*inner)?;
                if let Exp::Open { .. } = e {
                    Ok(Exp::Reg(self.exp_to_anyreg(e)?))
                } else {
                    Ok(e)
                }
            }
            Expr::Index { obj, key } => self.index_expr(*obj, *key),
            Expr::UnOp { op, operand, line } => {
                let (op, operand, line) = (*op, *operand, *line);
                self.unop(op, operand, line)
            }
            Expr::BinOp { op, lhs, rhs, line } => {
                let (op, lhs, rhs, line) = (*op, *lhs, *rhs, *line);
                self.binop(op, lhs, rhs, line)
            }
            Expr::Table { line, .. } => {
                let line = *line;
                self.table_ctor(id, line)
            }
            Expr::Vararg => self.vararg_expr(),
            Expr::Call { .. } | Expr::MethodCall { .. } => self.call_expr(id),
            Expr::Function(body) => self.function_exp(body, false),
        }
    }

    pub(super) fn vararg_expr(&mut self) -> Result<Exp, SyntaxError> {
        if !self.lr().is_vararg {
            return Err(self.err(self.last_line, "cannot use '...' outside a vararg function"));
        }
        let base = self.reserve(1)?;
        let pc = self.emit(Inst::iabc(Op::Vararg, base, 0, 2, false));
        Ok(Exp::Open { pc, base })
    }

    pub(super) fn call_expr(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        let ast = self.ast;
        match ast.expr(id) {
            Expr::Call { func, args, line } => {
                let (func, line) = (*func, *line);
                let base = self.lr().freereg;
                let fe = self.expr(func)?;
                self.set_freereg(base);
                let r = self.exp_to_nextreg(fe)?;
                debug_assert_eq!(r, base);
                let (nfixed, open) = self.args_onto_stack(self.ls(*args), base + 1)?;
                self.last_line = line;
                let b = if open { 0 } else { nfixed + 1 };
                let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
                self.set_freereg(base + 1);
                Ok(Exp::Open { pc, base })
            }
            Expr::MethodCall {
                obj,
                method,
                args,
                line,
            } => {
                let (obj, line) = (*obj, *line);
                let base = self.lr().freereg;
                let oe = self.expr(obj)?;
                let o = self.exp_to_anyreg(oe)?;
                self.set_freereg(base);
                self.reserve(2)?;
                let c = self.sym_const(method.sym);
                self.last_line = line;
                if c <= 0xFF {
                    self.emit(Inst::iabc(Op::SelfOp, base, o, c, true));
                } else if self.version >= LuaVersion::Lua55 {
                    // PUC 5.5 drops back to a plain GETTABLE when the SELF
                    // C-operand can't fit the constant — getobjname then
                    // classifies the call as "field". 5.5 errors.lua :328
                    // bakes the wording in (its comment literally says
                    // "cannot use 'self' opcode").
                    self.emit(Inst::iabc(Op::Move, base + 1, o, 0, false));
                    let kr = self.reserve(1)?;
                    self.load_const(kr, c);
                    self.emit(Inst::iabc(Op::GetTable, base, base + 1, kr, false));
                    self.set_freereg(base + 2);
                } else {
                    // PUC 5.4 `luaK_exp2RK`: load the key into a register and
                    // emit OP_SELF against it (k=false). The SELF tag stays on
                    // the instruction so getobjname classifies the call as
                    // "method" — 5.4 errors.lua :303 exercises this path.
                    let kr = self.reserve(1)?;
                    self.load_const(kr, c);
                    self.set_freereg(base);
                    self.reserve(2)?;
                    self.emit(Inst::iabc(Op::SelfOp, base, o, kr, false));
                }
                let (nfixed, open) = self.args_onto_stack(self.ls(*args), base + 2)?;
                self.last_line = line;
                let b = if open { 0 } else { nfixed + 2 };
                let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
                self.set_freereg(base + 1);
                Ok(Exp::Open { pc, base })
            }
            _ => unreachable!(),
        }
    }

    /// Stack call arguments at consecutive registers from `argbase`.
    /// Returns (fixed_arg_count, last_is_open).
    pub(super) fn args_onto_stack(
        &mut self,
        args: &[ExprId],
        argbase: u32,
    ) -> Result<(u32, bool), SyntaxError> {
        for (i, &a) in args.iter().enumerate() {
            let dst = argbase + i as u32;
            if dst >= max_regs(self.version) {
                // PUC `checkstack` raises "function or expression needs too
                // many registers" once the per-function register cap is hit;
                // a too-wide call site is just one path into it (errors.lua
                // :740 checkmessage "too many registers").
                return Err(self.regs_error(self.last_line));
            }
            self.set_freereg(dst);
            let last = i == args.len() - 1;
            let e = self.expr(a)?;
            if last && let Exp::Open { pc, base } = e {
                debug_assert_eq!(base, dst);
                self.patch_wanted(pc, 0);
                return Ok((args.len() as u32 - 1, true));
            }
            self.set_freereg(dst);
            let got = self.exp_to_nextreg(e)?;
            debug_assert_eq!(got, dst);
        }
        Ok((args.len() as u32, false))
    }

    /// Materialize into a specific register.
    pub(super) fn exp_to_reg(&mut self, e: Exp, reg: u32) -> Result<(), SyntaxError> {
        match e {
            Exp::Nil => {
                self.emit(Inst::iabc(Op::LoadNil, reg, 0, 0, false));
            }
            Exp::True => {
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
            }
            Exp::False => {
                self.emit(Inst::iabc(Op::LoadFalse, reg, 0, 0, false));
            }
            Exp::Int(i) => {
                if (-65535..=65535).contains(&i) {
                    self.emit(Inst::iasbx(Op::LoadI, reg, i as i32));
                } else {
                    let c = self.const_idx(ConstKey::Int(i), Value::Int(i));
                    self.load_const(reg, c);
                }
            }
            Exp::Float(mut f) => {
                if f == 0.0 && self.version == LuaVersion::Lua51 {
                    f = *self.l().zero_51.get_or_insert(f);
                }
                let as_int = f as i32;
                // bit-compare so the LoadF fast path doesn't fold -0.0 to +0.0
                // (`-0.0 == 0.0` but their bit patterns differ)
                if (-65535..=65535).contains(&as_int) && (as_int as f64).to_bits() == f.to_bits() {
                    self.emit(Inst::iasbx(Op::LoadF, reg, as_int));
                } else {
                    let c = self.const_idx(ConstKey::Float(f.to_bits()), Value::Float(f));
                    self.load_const(reg, c);
                }
            }
            Exp::Const(c) => self.load_const(reg, c),
            Exp::Reg(r) => {
                if r != reg {
                    self.emit(Inst::iabc(Op::Move, reg, r, 0, false));
                }
            }
            Exp::Reloc(pc) => self.patch_dest(pc, reg),
            Exp::Cmp { op, l, r, c } => {
                self.emit(Inst::iabc(op, l, r, c, true));
                self.emit(Inst::isj(Op::Jmp, 1));
                self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                let tpad = self.here();
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
                // Jmp(1) above skips the LFalseSkip and lands on the LoadTrue
                // pad — that pc is a jump destination.
                self.mark_target(tpad);
            }
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                if base != reg {
                    self.emit(Inst::iabc(Op::Move, reg, base, 0, false));
                }
            }
        }
        Ok(())
    }

    pub(super) fn exp_to_nextreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        let reg = self.reserve(1)?;
        self.exp_to_reg(e, reg)?;
        Ok(reg)
    }

    pub(super) fn exp_to_anyreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        match e {
            Exp::Reg(r) => Ok(r),
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                Ok(base)
            }
            e => self.exp_to_nextreg(e),
        }
    }

    /// 5.1: zeros of a condition the parser folded away still entered the
    /// constant table (see `Level::zero_51`).
    pub(super) fn note_zeros(&mut self, zeros: &[f64]) {
        if let Some(&z) = zeros.first() {
            self.l().zero_51.get_or_insert(z);
        }
    }
}
