//! Operand checks: the registers, constants, upvalues and nested
//! functions each instruction names (see the parent module's docs).

use super::Checker;
use crate::vm::isa::Op;

impl Checker<'_> {
    /// Registers `first .. first + len` must lie below `max_stack`.
    fn regs(&self, pc: usize, first: u32, len: u32) -> Result<(), String> {
        if first + len > self.max() {
            let what = if len <= 1 {
                format!("register {first}")
            } else {
                format!("registers {first}..{}", first + len - 1)
            };
            return Err(self.err(
                pc,
                format!("{what} out of range (stack size {})", self.max()),
            ));
        }
        Ok(())
    }

    fn reg(&self, pc: usize, r: u32) -> Result<(), String> {
        self.regs(pc, r, 1)
    }

    fn konst(&self, pc: usize, k: u32) -> Result<(), String> {
        let n = self.p.consts.len();
        if k as usize >= n {
            return Err(self.err(pc, format!("constant {k} out of range ({n} constants)")));
        }
        Ok(())
    }

    fn upval(&self, pc: usize, u: u32) -> Result<(), String> {
        let n = self.p.upvals.len();
        if u as usize >= n {
            return Err(self.err(pc, format!("upvalue {u} out of range ({n} upvalues)")));
        }
        Ok(())
    }

    pub(super) fn check_operands(&self, pc: usize) -> Result<(), String> {
        let i = self.inst(pc);
        let (a, b, c) = (i.a(), i.b(), i.c());
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
            Op::GetTabUp => {
                self.reg(pc, a)?;
                self.upval(pc, b)?;
                self.konst(pc, c)
            }
            Op::GetTable | Op::SetTable => {
                self.reg(pc, a)?;
                self.reg(pc, b)?;
                self.reg(pc, c)
            }
            Op::GetField => {
                self.reg(pc, a)?;
                self.reg(pc, b)?;
                self.konst(pc, c)
            }
            Op::SetTabUp => {
                self.upval(pc, a)?;
                self.konst(pc, b)?;
                self.reg(pc, c)
            }
            Op::SetI => {
                self.reg(pc, a)?;
                self.reg(pc, c)
            }
            Op::SetField => {
                self.reg(pc, a)?;
                self.konst(pc, b)?;
                self.reg(pc, c)
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
                    self.konst(pc, c)
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
            Op::Concat => {
                if b < 2 {
                    return Err(self.err(pc, format!("concatenates {b} values")));
                }
                self.regs(pc, a, b)
            }
            Op::Jmp | Op::Return0 | Op::ExtraArg => Ok(()),
            Op::Eq | Op::Lt | Op::Le => {
                self.reg(pc, a)?;
                self.reg(pc, b)
            }
            Op::EqK => {
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
            // four hidden slots A..A+3
            Op::ForPrep | Op::ForLoop => self.regs(pc, a, 4),
            // A..A+3 hidden, loop variables from A+4
            Op::TForPrep => self.regs(pc, a, 4),
            Op::TForCall => {
                if c == 0 {
                    return Err(self.err(pc, "no loop variables".to_string()));
                }
                self.regs(pc, a, 4 + c)
            }
            Op::TForLoop => self.regs(pc, a, 5),
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
