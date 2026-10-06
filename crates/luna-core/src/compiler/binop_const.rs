//! The constant- and immediate-operand instruction forms of the binary
//! operators.

use super::*;

/// An operand encoded in its instruction rather than read from a register.
#[derive(Clone, Copy)]
pub(super) enum Operand {
    /// the constant- or immediate-operand arithmetic opcode and its C field
    Arith(Op, u32),
    /// the comparison opcode with its B and C fields
    Cmp(Op, u32, u32),
}

impl Compiler<'_> {
    /// The number `e` as a constant-table index that fits an instruction's
    /// 8-bit field.
    pub(super) fn num_const(&mut self, e: &Exp) -> Option<u32> {
        let c = match *e {
            Exp::Int(i) => self.const_idx(Value::Int(i)),
            Exp::Float(mut f) => {
                if f == 0.0 && self.version == LuaVersion::Lua51 {
                    f = *self.l().zero_51.get_or_insert(f);
                }
                self.const_idx(Value::Float(f))
            }
            _ => return None,
        };
        (c <= MAX_C).then_some(c)
    }

    /// How `e` goes into the instruction of `op` instead of a register
    /// (PUC `codearith` / `codebitwise` / `codeorder` / `codeeq`), if it does.
    /// `left`: `e` is the left operand.
    pub(super) fn const_operand(&mut self, op: BinOp, e: &Exp, left: bool) -> Option<Operand> {
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
        // an immediate is a constant of the dialects before 5.4: it enters
        // the table here, where PUC's code generator adds it
        let immediate = matches!(
            form,
            Operand::Arith(Op::AddI | Op::SubI | Op::ShlI | Op::ShrI, _)
                | Operand::Cmp(Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI, ..)
        );
        if immediate {
            match *e {
                Exp::Int(i) => self.number_const_before_54(Value::Int(i)),
                Exp::Float(f) => self.number_const_before_54(Value::Float(f)),
                _ => {}
            }
        }
        Some(form)
    }

    /// Emit `op` on register `reg` and the in-instruction operand `form`;
    /// `flip`: the operand was written on the left.
    pub(super) fn emit_const_operand(
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
}
