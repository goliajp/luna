//! Binary operators: operand evaluation order, the constant- and
//! immediate-operand instruction forms, and comparisons.

use super::*;

/// An operand encoded in its instruction rather than read from a register.
#[derive(Clone, Copy)]
enum Operand {
    /// the constant- or immediate-operand arithmetic opcode and its C field
    Arith(Op, u32),
    /// the comparison opcode with its B and C fields
    Cmp(Op, u32, u32),
}

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
        // A numeral on the left of a commutative operator or of a comparison
        // stays out of a register until the right operand is known: it may
        // become the instruction's own operand (PUC `luaK_infix`).
        let left_numeral = deferred.is_none()
            && matches!(le, Exp::Int(_) | Exp::Float(_))
            && matches!(
                op,
                BinOp::Add
                    | BinOp::Mul
                    | BinOp::BAnd
                    | BinOp::BOr
                    | BinOp::BXor
                    | BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Le
                    | BinOp::Gt
                    | BinOp::Ge
            );
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
        // and the side it was written on. Two numerals the fold left alone
        // (`1 // 0`): the left one takes a register after all.
        let mut in_inst: Option<(Operand, bool)> = None;
        if l.is_none() {
            if matches!(re, Exp::Int(_) | Exp::Float(_) | Exp::Const(_)) {
                let saved_line = self.force_line.replace(line);
                let reg = self.exp_to_anyreg(le)?;
                self.force_line = saved_line;
                if reg >= saved {
                    self.set_freereg(reg + 1);
                }
                l = Some(reg);
            } else if let Some(form) = self.const_operand(op, &le, true, saved) {
                in_inst = Some((form, true));
            }
        }
        if l.is_some()
            && !sub_zero
            && let Some(form) = self.const_operand(op, &re, false, saved)
        {
            in_inst = Some((form, false));
        }
        // the register operand(s)
        let (l, r) = match (l, in_inst) {
            (Some(l), Some(_)) => (l, 0),
            (Some(l), None) => (l, self.exp_to_anyreg(re)?),
            (None, _) => {
                let r = self.exp_to_anyreg(re)?;
                if in_inst.is_some() {
                    (r, 0)
                } else {
                    // the left numeral has no in-instruction form here
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

    /// The number `e` as a constant-table index that fits an instruction's
    /// 8-bit field.
    fn num_const(&mut self, e: &Exp) -> Option<u32> {
        let c = match *e {
            Exp::Int(i) => self.const_idx(ConstKey::Int(i), Value::Int(i)),
            Exp::Float(mut f) => {
                if f == 0.0 && self.version == LuaVersion::Lua51 {
                    f = *self.l().zero_51.get_or_insert(f);
                }
                self.const_idx(ConstKey::Float(f.to_bits()), Value::Float(f))
            }
            _ => return None,
        };
        (c <= MAX_C).then_some(c)
    }

    /// How `e` goes into the instruction of `op` instead of a register
    /// (PUC `codearith` / `codebitwise` / `codeorder` / `codeeq`), if it does.
    /// `left`: `e` is the left operand. `saved` is the first free register
    /// once both operands are released.
    fn const_operand(&mut self, op: BinOp, e: &Exp, left: bool, saved: u32) -> Option<Operand> {
        // The instruction must leave the frame's last register unused: a
        // trace recording loads the operand there (`trace_record_push`). The
        // operand and result registers are at most `saved`.
        if saved + 2 > max_regs(self.version) {
            return None;
        }
        let imm = match *e {
            Exp::Int(i) if (MIN_SC as i64..=MAX_SC as i64).contains(&i) => Some((i as i32, false)),
            _ => None,
        };
        // a comparison also takes a float with a small integer value
        let cmp_imm = imm.or(match *e {
            Exp::Float(f) if f.fract() == 0.0 && (MIN_SC as f64..=MAX_SC as f64).contains(&f) => {
                Some((f as i32, true))
            }
            _ => None,
        });
        let enc = |i: i32| (i + OFFSET_SC) as u32;
        let form = match op {
            BinOp::Add => match imm {
                Some((i, _)) => Operand::Arith(Op::AddI, enc(i)),
                None => Operand::Arith(Op::AddK, self.num_const(e)?),
            },
            BinOp::Sub if !left => match imm {
                Some((i, _)) => Operand::Arith(Op::SubI, enc(i)),
                None => Operand::Arith(Op::SubK, self.num_const(e)?),
            },
            BinOp::Mul => Operand::Arith(Op::MulK, self.num_const(e)?),
            BinOp::Mod if !left => Operand::Arith(Op::ModK, self.num_const(e)?),
            BinOp::Pow if !left => Operand::Arith(Op::PowK, self.num_const(e)?),
            BinOp::Div if !left => Operand::Arith(Op::DivK, self.num_const(e)?),
            BinOp::IDiv if !left => Operand::Arith(Op::IDivK, self.num_const(e)?),
            BinOp::BAnd | BinOp::BOr | BinOp::BXor if matches!(e, Exp::Int(_)) => {
                let k = self.num_const(e)?;
                Operand::Arith(
                    match op {
                        BinOp::BAnd => Op::BAndK,
                        BinOp::BOr => Op::BOrK,
                        _ => Op::BXorK,
                    },
                    k,
                )
            }
            BinOp::Shl if !left => Operand::Arith(Op::ShlI, enc(imm?.0)),
            BinOp::Shr if !left => Operand::Arith(Op::ShrI, enc(imm?.0)),
            BinOp::Eq | BinOp::Ne => match (cmp_imm, e) {
                (Some((i, f)), _) => Operand::Cmp(Op::EqI, enc(i), f as u32),
                (None, &Exp::Const(c)) if c <= MAX_B => Operand::Cmp(Op::EqK, c, 0),
                _ => Operand::Cmp(Op::EqK, self.num_const(e)?, 0),
            },
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let (i, f) = cmp_imm?;
                // `K < x` is `x > K`
                let cmp = match (op, left) {
                    (BinOp::Lt, false) | (BinOp::Gt, true) => Op::LtI,
                    (BinOp::Le, false) | (BinOp::Ge, true) => Op::LeI,
                    (BinOp::Gt, false) | (BinOp::Lt, true) => Op::GtI,
                    _ => Op::GeI,
                };
                Operand::Cmp(cmp, enc(i), f as u32)
            }
            _ => return None,
        };
        let lvl = self.l();
        lvl.max_stack = lvl.max_stack.max(saved + 2);
        Some(form)
    }

    /// Emit `op` on register `reg` and the in-instruction operand `form`;
    /// `flip`: the operand was written on the left.
    fn emit_const_operand(
        &mut self,
        op: BinOp,
        reg: u32,
        form: Operand,
        flip: bool,
    ) -> Result<Exp, SyntaxError> {
        Ok(match form {
            Operand::Arith(kop, c) => Exp::Reloc(self.emit(Inst::iabc(kop, 0, reg, c, flip))),
            Operand::Cmp(cop, b, c) if op == BinOp::Ne => self.negate_cmp(cop, reg, b, c)?,
            Operand::Cmp(cop, b, c) => Exp::Cmp {
                op: cop,
                l: reg,
                r: b,
                c,
            },
        })
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
