//! Operand checks: the registers, constants, upvalues and nested
//! functions each instruction names (see the parent module's docs).

use super::Checker;
use crate::vm::isa::Op;

impl Checker<'_> {
    pub(super) fn check_operands(&self, pc: usize) -> Result<(), String> {
        let i = self.inst(pc);
        let (a, b, c) = (i.a(), i.b(), i.c());
        let var = i.op().for_layout().map_or(0, |l| l.var());
        match self.ops[pc] {
            Op::Move | Op::Unm | Op::BNot | Op::Not | Op::Len | Op::GetI | Op::TestSet => {
                self.reg(pc, a)?;
                self.reg(pc, b)
            }
            Op::LoadI
            | Op::LoadF
            | Op::LoadFalse
            | Op::LFalseSkip
            | Op::LoadTrue
            | Op::NewTable
            | Op::Test
            | Op::Close
            | Op::Tbc
            | Op::GetVarg
            | Op::Return1 => self.reg(pc, a),
            Op::LoadK => {
                self.reg(pc, a)?;
                self.konst(pc, i.bx())
            }
            Op::LoadKx => {
                self.reg(pc, a)?;
                match self.p.code.get(pc + 1) {
                    Some(x) if x.0 & 0x7F == Op::ExtraArg as u32 => self.konst(pc, x.ax()),
                    _ => Err(self.err(pc, "not followed by its extra argument".to_string())),
                }
            }
            Op::LoadNil => self.regs(pc, a, b + 1),
            Op::GetUpval | Op::SetUpval => {
                self.reg(pc, a)?;
                self.upval(pc, b)
            }
            Op::GetGlobal | Op::SetGlobal => {
                self.reg(pc, a)?;
                self.upval(pc, 0)?;
                self.kstr(pc, i.bx())
            }
            Op::GetTabUp => {
                self.reg(pc, a)?;
                self.upval(pc, b)?;
                self.kstr(pc, c)
            }
            Op::GetTable => {
                self.reg(pc, a)?;
                self.reg(pc, b)?;
                self.reg(pc, c)
            }
            Op::SetTable => {
                self.reg(pc, a)?;
                self.reg(pc, b)?;
                self.value(pc, i.k(), c)
            }
            Op::GetTableK => {
                self.reg(pc, a)?;
                self.reg(pc, b)?;
                self.konst(pc, c)
            }
            Op::SetTableK => {
                self.reg(pc, a)?;
                self.konst(pc, b)?;
                self.value(pc, i.k(), c)
            }
            Op::GetField => {
                self.reg(pc, a)?;
                self.reg(pc, b)?;
                self.kstr(pc, c)
            }
            Op::GetTabUpR => {
                self.reg(pc, a)?;
                self.upval(pc, b)?;
                self.value(pc, i.k(), c)
            }
            Op::SetTabUpR => {
                self.upval(pc, a)?;
                self.reg(pc, b)?;
                self.value(pc, i.k(), c)
            }
            Op::SetTabUpK => {
                self.upval(pc, a)?;
                self.konst(pc, b)?;
                self.value(pc, i.k(), c)
            }
            Op::SetTabUp => {
                self.upval(pc, a)?;
                self.kstr(pc, b)?;
                self.value(pc, i.k(), c)
            }
            Op::SetI => {
                self.reg(pc, a)?;
                self.value(pc, i.k(), c)
            }
            Op::SetField => {
                self.reg(pc, a)?;
                self.kstr(pc, b)?;
                self.value(pc, i.k(), c)
            }
            Op::SetList => {
                self.regs(pc, a, b + 1)?;
                if i.k() && self.ops.get(pc + 1) != Some(&Op::ExtraArg) {
                    return Err(self.err(pc, "not followed by its extra argument".to_string()));
                }
                Ok(())
            }
            Op::SelfOp => {
                self.regs(pc, a, 2)?;
                self.reg(pc, b)?;
                if i.k() {
                    self.kstr(pc, c)
                } else {
                    self.reg(pc, c)
                }
            }
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Mod
            | Op::Pow
            | Op::Div
            | Op::IDiv
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr => {
                self.reg(pc, a)?;
                self.reg(pc, b)?;
                self.reg(pc, c)
            }
            Op::AddI | Op::SubI | Op::ShrI | Op::ShlI => {
                self.reg(pc, a)?;
                self.reg(pc, b)
            }
            Op::AddK
            | Op::SubK
            | Op::MulK
            | Op::ModK
            | Op::PowK
            | Op::DivK
            | Op::IDivK
            | Op::BAndK
            | Op::BOrK
            | Op::BXorK
            | Op::ShlK
            | Op::ShrK => {
                self.reg(pc, a)?;
                self.reg(pc, b)?;
                self.konst(pc, c)
            }
            Op::AddKK
            | Op::SubKK
            | Op::MulKK
            | Op::ModKK
            | Op::PowKK
            | Op::DivKK
            | Op::IDivKK
            | Op::BAndKK
            | Op::BOrKK
            | Op::BXorKK
            | Op::ShlKK
            | Op::ShrKK => {
                self.reg(pc, a)?;
                self.konst(pc, b)?;
                self.konst(pc, c)
            }
            Op::EqKK | Op::LtKK | Op::LeKK => {
                self.konst(pc, a)?;
                self.konst(pc, b)
            }
            Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI => self.reg(pc, a),
            Op::Concat => {
                if b < 2 {
                    return Err(self.err(pc, format!("concatenates {b} values")));
                }
                self.regs(pc, a, b)
            }
            Op::Jmp | Op::ExtraArg => Ok(()),
            // the first register past the returning frame's locals: at most
            // one past its last register
            Op::Return0 => self.regs(pc, a, 0),
            Op::Eq | Op::Lt | Op::Le => {
                self.reg(pc, a)?;
                self.reg(pc, b)
            }
            Op::EqK | Op::LtK | Op::LeK => {
                self.reg(pc, a)?;
                self.konst(pc, b)
            }
            // function + fixed arguments; fixed results from A
            Op::Call => {
                self.regs(pc, a, b.max(1))?;
                self.regs(pc, a, c.saturating_sub(1))
            }
            Op::TailCall => self.regs(pc, a, b.max(1)),
            // B - 1 values from A; A itself may sit at the frame's top when
            // none are returned
            Op::Return => {
                if b == 0 {
                    self.regs(pc, a, 0)
                } else {
                    self.regs(pc, a, b - 1)
                }
            }
            // the hidden slots and the first loop variable
            Op::ForPrep | Op::ForLoop | Op::ForPrep55 | Op::ForLoop55 => self.regs(pc, a, var + 1),
            Op::TForPrep | Op::TForPrep53 | Op::TForPrep55 => self.regs(pc, a, var),
            // the call runs on three copies at the first variable, which
            // its results replace
            Op::TForCall | Op::TForCall53 | Op::TForCall55 => {
                if c == 0 {
                    return Err(self.err(pc, "no loop variables".to_string()));
                }
                self.regs(pc, a, var + c.max(3))
            }
            Op::TForLoop | Op::TForLoop53 | Op::TForLoop55 => self.regs(pc, a, var + 1),
            Op::Closure => {
                self.reg(pc, a)?;
                let n = self.p.protos.len();
                if i.bx() as usize >= n {
                    return Err(self.err(
                        pc,
                        format!("function {} out of range ({n} functions)", i.bx()),
                    ));
                }
                Ok(())
            }
            Op::Vararg => {
                if c == 0 {
                    self.regs(pc, a, 0)
                } else {
                    self.regs(pc, a, c - 1)
                }
            }
            Op::VargIdx => {
                self.reg(pc, a)?;
                self.reg(pc, c)
            }
            Op::ErrNNil => {
                self.reg(pc, a)?;
                match i.bx() {
                    0 => Ok(()),
                    bx => self.konst(pc, bx - 1),
                }
            }
        }
    }
}
