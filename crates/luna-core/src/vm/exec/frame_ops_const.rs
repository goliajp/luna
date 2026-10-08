//! The operators on two constants and the shifts by a constant: 5.1–5.3
//! code that has them is rare, so they run out of line and keep the fast
//! loop small.

use super::*;

impl Vm {
    /// `AddKK` … `ShrKK`, `EqKK`, `LtKK`, `LeKK`, `ShlK`, `ShrK`.
    pub(super) fn const_frame_op(
        &mut self,
        inst: Inst,
        cl: Gc<LuaClosure>,
        base: u32,
    ) -> Result<(), LuaError> {
        let k = |i: u32| cl.proto.consts[i as usize];
        let arith = match inst.op() {
            Op::EqKK => {
                self.cond_skip(k(inst.a()).raw_eq(k(inst.b())), inst.k());
                return Ok(());
            }
            op @ (Op::LtKK | Op::LeKK) => {
                let (l, r) = (k(inst.a()), k(inst.b()));
                let step = self.less_step(l, r, op == Op::LeKK)?;
                return self.op_compare(step, l, r, inst.k());
            }
            // `k`: the constant is the left operand
            op @ (Op::ShlK | Op::ShrK) => {
                let (x, c) = (self.r(base, inst.b()), k(inst.c()));
                let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                let aop = if op == Op::ShlK {
                    ArithOp::Shl
                } else {
                    ArithOp::Shr
                };
                return self.arith_slow(inst.a(), base, aop, l, r);
            }
            Op::AddKK => ArithOp::Add,
            Op::SubKK => ArithOp::Sub,
            Op::MulKK => ArithOp::Mul,
            Op::ModKK => ArithOp::Mod,
            Op::PowKK => ArithOp::Pow,
            Op::DivKK => ArithOp::Div,
            Op::IDivKK => ArithOp::IDiv,
            Op::BAndKK => ArithOp::BAnd,
            Op::BOrKK => ArithOp::BOr,
            Op::BXorKK => ArithOp::BXor,
            Op::ShlKK => ArithOp::Shl,
            Op::ShrKK => ArithOp::Shr,
            op => unreachable!("{op:?} is not a constant op"),
        };
        self.arith_slow(inst.a(), base, arith, k(inst.b()), k(inst.c()))
    }
}
