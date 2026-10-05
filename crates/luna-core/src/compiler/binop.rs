//! Binary operators: operand evaluation order and comparisons (the
//! constant- and immediate-operand forms are in `binop_const`).

use super::binop_const::Operand;
use super::*;

impl Compiler<'_> {
    pub(super) fn binop(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        match op {
            BinOp::And | BinOp::Or => return self.and_or(op, lhs, rhs, line),
            BinOp::Concat => return self.concat(lhs, rhs, line),
            _ => {}
        }
        let saved = self.lr().freereg;
        // PUC's `infix` discharges the left operand *after* consuming the
        // operator token (luaK_indexed → luaK_exp2anyreg called from infix),
        // so any GET emitted for the lhs lands on the operator's line, not
        // on the line of the lhs itself (db.lua :193 line-trace family).
        // luna parses the lhs ahead of time; pin the line through
        // `force_line` for the duration of the lhs walk so every emit it
        // performs is attributed to the operator's line, then drop the pin
        // before parsing the rhs (which discharges at its own last-token
        // line, matching PUC).
        let saved_force = self.force_line.replace(line);
        // a 5.1 left operand whose logic folds away (see `numeral`)
        let mut zeros = Vec::new();
        let le = match numeral(self.ast, lhs, self.version, &mut zeros) {
            Some(n) if self.version == LuaVersion::Lua51 && is_logical(self.ast, lhs) => {
                self.note_zeros(&zeros);
                match n {
                    Num::Int(i) => Exp::Int(i),
                    Num::Float(f) => Exp::Float(f),
                }
            }
            _ => self.expr(lhs)?,
        };
        zeros.clear();
        if let Some(folded) = fold_arith(op, &le, self.ast, rhs, self.version, &mut zeros) {
            self.note_zeros(&zeros);
            self.force_line = saved_force;
            return Ok(folded);
        }
        // PUC 5.1 puts a numeral left operand in the constant table only
        // after the right operand (`luaK_infix` leaves numerals be, and
        // `codearith` takes the right one first). For a zero that order
        // decides which sign the function's zeros share (see `zero_51`).
        let deferred = match le {
            Exp::Float(f) if f == 0.0 && self.version == LuaVersion::Lua51 => Some(f),
            _ => None,
        };
        // A numeral on the left stays out of a register until the right
        // operand is known: it may become the instruction's own operand, and
        // otherwise it is loaded after the right one (PUC `luaK_infix`).
        let left_numeral = deferred.is_none() && matches!(le, Exp::Int(_) | Exp::Float(_));
        let mut l = if left_numeral {
            None
        } else {
            Some(match deferred {
                Some(_) => self.reserve(1)?,
                None => self.exp_to_anyreg(le)?,
            })
        };
        self.force_line = saved_force;
        // Protect the left operand's register if it is a fresh temporary at the
        // top of the stack (e.g. a CONCAT result): evaluating the right operand
        // must not reuse and clobber it before the binary op reads it.
        if let Some(l) = l
            && l >= saved
        {
            self.set_freereg(l + 1);
        }
        // 5.4+ compiles `x - K` for a small integer constant K as `x + -K`
        // (`ADDI`). That is the same number except for K = 0, where
        // `-0.0 - 0` becomes `-0.0 + 0`, which is `0.0`. K is whatever
        // PUC's parser folds to a constant: `(0)`, `1 - 1`, `5 % 5`...
        let sub_zero = op == BinOp::Sub && self.version >= LuaVersion::Lua54 && {
            let ast = self.ast;
            matches!(
                ct_operand(ast, rhs, &mut |name| self.ct_const_named(self.nm(name))),
                Some(CtConst::Int(0))
            )
        };
        let re = self.expr(rhs)?;
        // The operand that goes into the instruction instead of a register,
        // and the side it was written on.
        let mut in_inst: Option<(Operand, bool)> = None;
        let mut right_reg = None;
        if l.is_none() {
            if matches!(re, Exp::Int(_) | Exp::Float(_) | Exp::Const(_)) {
                // Two numerals the fold left alone (`7.5 // 0`), as PUC's
                // `codearith` takes them: 5.4+ moves the left one of `+`, `*`
                // (and an integer of a bitwise operator) to the right; the
                // right one becomes the operand, its constant first, and the
                // left one a register; with no such form the right one takes
                // its register first.
                let swap = self.version >= LuaVersion::Lua54
                    && (matches!(op, BinOp::Add | BinOp::Mul)
                        || matches!(op, BinOp::BAnd | BinOp::BOr | BinOp::BXor)
                            && matches!(le, Exp::Int(_)));
                let saved_line = self.force_line.replace(line);
                if swap && let Some(form) = self.const_operand(op, &le, true, saved) {
                    in_inst = Some((form, true));
                    right_reg = Some(self.exp_to_anyreg(re)?);
                } else if !sub_zero && let Some(form) = self.const_operand(op, &re, false, saved) {
                    in_inst = Some((form, false));
                    l = Some(self.exp_to_anyreg(le)?);
                } else {
                    let r = self.exp_to_anyreg(re)?;
                    if r >= saved {
                        self.set_freereg(r + 1);
                    }
                    right_reg = Some(r);
                    l = Some(self.exp_to_anyreg(le)?);
                }
                self.force_line = saved_line;
            } else if let Some(form) = self.const_operand(op, &le, true, saved) {
                in_inst = Some((form, true));
            }
        }
        if l.is_some()
            && right_reg.is_none()
            && in_inst.is_none()
            && !sub_zero
            && let Some(form) = self.const_operand(op, &re, false, saved)
        {
            in_inst = Some((form, false));
        }
        // the register operand(s)
        let (l, r) = match (l, in_inst, right_reg) {
            // swapped: the register is the right operand's
            (_, Some((_, true)), Some(r)) => (r, 0),
            (Some(l), None, Some(r)) => (l, r),
            (Some(l), Some(_), _) => (l, 0),
            (Some(l), None, None) => (l, self.exp_to_anyreg(re)?),
            (None, _, _) => {
                let r = self.exp_to_anyreg(re)?;
                if in_inst.is_some() {
                    (r, 0)
                } else {
                    // the left numeral has no in-instruction form here: it
                    // goes above the right operand, which it must not clobber
                    if r >= saved {
                        self.set_freereg(r + 1);
                    }
                    let saved_line = self.force_line.replace(line);
                    let l = self.exp_to_anyreg(le)?;
                    self.force_line = saved_line;
                    (l, r)
                }
            }
        };
        if let Some(f) = deferred {
            let saved_line = self.force_line.replace(line);
            self.exp_to_reg(Exp::Float(f), l)?;
            self.force_line = saved_line;
        }
        self.set_freereg(saved);
        // PUC attributes the arith op itself to the operator's line, but
        // leaves `lastline` at the rhs's last token (so a following SETTABUP
        // / SETUPVAL for the assignment lands on the rhs's end line, not the
        // operator). Pin the line for the arith emit, but don't stomp
        // `last_line` permanently.
        let saved_force_arith = self.force_line.replace(line);
        let r_op = match in_inst {
            Some((form, flip)) => self.emit_const_operand(op, l, form, flip),
            None => self.emit_binop(op, l, r, sub_zero),
        };
        self.force_line = saved_force_arith;
        r_op
    }

    /// Emit `op` on the registers `l` and `r`.
    fn emit_binop(
        &mut self,
        op: BinOp,
        l: u32,
        r: u32,
        sub_zero: bool,
    ) -> Result<Exp, SyntaxError> {
        Ok(match op {
            BinOp::Add => self.arith(Op::Add, l, r),
            BinOp::Sub if sub_zero => Exp::Reloc(self.emit(Inst::iabc(Op::Add, 0, l, r, true))),
            BinOp::Sub => self.arith(Op::Sub, l, r),
            BinOp::Mul => self.arith(Op::Mul, l, r),
            BinOp::Div => self.arith(Op::Div, l, r),
            BinOp::IDiv => self.arith(Op::IDiv, l, r),
            BinOp::Mod => self.arith(Op::Mod, l, r),
            BinOp::Pow => self.arith(Op::Pow, l, r),
            BinOp::BAnd => self.arith(Op::BAnd, l, r),
            BinOp::BOr => self.arith(Op::BOr, l, r),
            BinOp::BXor => self.arith(Op::BXor, l, r),
            BinOp::Shl => self.arith(Op::Shl, l, r),
            BinOp::Shr => self.arith(Op::Shr, l, r),
            BinOp::Eq => Exp::Cmp {
                op: Op::Eq,
                l,
                r,
                c: 0,
            },
            BinOp::Ne => self.negate_cmp(Op::Eq, l, r, 0)?,
            BinOp::Lt => Exp::Cmp {
                op: Op::Lt,
                l,
                r,
                c: 0,
            },
            BinOp::Le => Exp::Cmp {
                op: Op::Le,
                l,
                r,
                c: 0,
            },
            BinOp::Gt => Exp::Cmp {
                op: Op::Lt,
                l: r,
                r: l,
                c: 0,
            },
            BinOp::Ge => Exp::Cmp {
                op: Op::Le,
                l: r,
                r: l,
                c: 0,
            },
            BinOp::And | BinOp::Or | BinOp::Concat => unreachable!(),
        })
    }

    fn arith(&mut self, op: Op, l: u32, r: u32) -> Exp {
        Exp::Reloc(self.emit(Inst::iabc(op, 0, l, r, false)))
    }

    /// `a ~= b`: comparison materialized with inverted k.
    pub(super) fn negate_cmp(
        &mut self,
        op: Op,
        l: u32,
        r: u32,
        c: u32,
    ) -> Result<Exp, SyntaxError> {
        let reg = self.reserve(1)?;
        self.l().freereg -= 1;
        self.emit(Inst::iabc(op, l, r, c, false));
        self.emit(Inst::isj(Op::Jmp, 1));
        self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
        let tpad = self.here();
        self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
        // Jmp(1) lands on tpad — mark.
        self.mark_target(tpad);
        Ok(Exp::Reg(reg))
    }
}
