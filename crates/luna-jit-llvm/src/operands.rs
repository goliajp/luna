//! How the compute path reads the operands of the arithmetic and
//! comparison opcodes: the register forms (`Add R, R`, `Lt R, R`) and the
//! constant- and immediate-operand forms the compiler emits in their
//! place (`AddI`, `AddK`, `LtI`, `EqK`, …). The path computes on integers
//! only, so a constant operand is lowered when it is an integer and the
//! function is refused otherwise, exactly as it was refused for the
//! `LoadF` / `LoadK` that used to precede the register form.

use inkwell::IntPredicate;
use luna_core::runtime::Value;
use luna_core::vm::isa::{Inst, Op};

/// One operand of an integer op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operand {
    Reg(u32),
    Imm(i64),
}

/// `R[A] := R[B] op <operand>` opcodes the compute path lowers.
pub(crate) fn is_arith(op: Op) -> bool {
    matches!(
        op,
        Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Mod
            | Op::AddI
            | Op::SubI
            | Op::AddK
            | Op::SubK
            | Op::MulK
            | Op::ModK
    )
}

/// `if ((R[A] cmp <operand>) ~= k) then pc++` opcodes the compute path
/// lowers; each is paired with the `Jmp` that follows it.
pub(crate) fn is_compare(op: Op) -> bool {
    matches!(
        op,
        Op::Lt | Op::Le | Op::Eq | Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI | Op::EqK
    )
}

/// The registers an arithmetic or comparison instruction reads; empty for
/// any other instruction. The `B` / `C` field of an immediate or constant
/// form is not a register.
pub(crate) fn operand_regs(ins: Inst) -> Vec<u32> {
    match ins.op() {
        Op::Add | Op::Sub | Op::Mul | Op::Mod => vec![ins.b(), ins.c()],
        Op::AddI | Op::SubI | Op::AddK | Op::SubK | Op::MulK | Op::ModK => vec![ins.b()],
        Op::Lt | Op::Le | Op::Eq => vec![ins.a(), ins.b()],
        Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI | Op::EqK => vec![ins.a()],
        _ => vec![],
    }
}

fn int_const(consts: &[Value], idx: u32) -> Option<i64> {
    match consts.get(idx as usize)? {
        Value::Int(i) => Some(*i),
        _ => None,
    }
}

/// The register-form operator and the two operands of an arithmetic
/// instruction, `None` when the instruction is not one the path lowers.
/// The result does not depend on `k` (the constant written on the left).
pub(crate) fn int_arith(ins: Inst, consts: &[Value]) -> Option<(Op, Operand, Operand)> {
    let lhs = Operand::Reg(ins.b());
    match ins.op() {
        Op::Add | Op::Sub | Op::Mul | Op::Mod => Some((ins.op(), lhs, Operand::Reg(ins.c()))),
        Op::AddI | Op::SubI => Some((
            ins.arith_const_op()?,
            lhs,
            Operand::Imm(i64::from(ins.sc())),
        )),
        Op::AddK | Op::SubK | Op::MulK => Some((
            ins.arith_const_op()?,
            lhs,
            Operand::Imm(int_const(consts, ins.c())?),
        )),
        // a zero divisor is the interpreter's error
        Op::ModK => match int_const(consts, ins.c())? {
            0 => None,
            k => Some((Op::Mod, lhs, Operand::Imm(k))),
        },
        _ => None,
    }
}

/// The predicate and right operand of a comparison instruction (the left
/// operand is `R[A]`), `None` when the instruction is not one the path
/// lowers: a float immediate (`C != 0`) or a non-integer constant.
pub(crate) fn int_compare(ins: Inst, consts: &[Value]) -> Option<(IntPredicate, Operand)> {
    let imm = Operand::Imm(i64::from(ins.sb()));
    match ins.op() {
        Op::Lt => Some((IntPredicate::SLT, Operand::Reg(ins.b()))),
        Op::Le => Some((IntPredicate::SLE, Operand::Reg(ins.b()))),
        Op::Eq => Some((IntPredicate::EQ, Operand::Reg(ins.b()))),
        Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI if ins.c() != 0 => None,
        Op::EqI => Some((IntPredicate::EQ, imm)),
        Op::LtI => Some((IntPredicate::SLT, imm)),
        Op::LeI => Some((IntPredicate::SLE, imm)),
        Op::GtI => Some((IntPredicate::SGT, imm)),
        Op::GeI => Some((IntPredicate::SGE, imm)),
        Op::EqK => Some((IntPredicate::EQ, Operand::Imm(int_const(consts, ins.b())?))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::vm::isa::OFFSET_SC;

    fn enc(i: i32) -> u32 {
        (i + OFFSET_SC) as u32
    }

    #[test]
    fn immediate_forms_carry_the_signed_field() {
        let consts = [Value::Int(100_000), Value::Float(1.5), Value::Nil];
        assert_eq!(
            int_arith(Inst::iabc(Op::AddI, 1, 0, enc(-3), false), &consts),
            Some((Op::Add, Operand::Reg(0), Operand::Imm(-3)))
        );
        assert_eq!(
            int_arith(Inst::iabc(Op::AddI, 1, 0, enc(7), true), &consts),
            Some((Op::Add, Operand::Reg(0), Operand::Imm(7)))
        );
        assert_eq!(
            int_arith(Inst::iabc(Op::MulK, 1, 0, 0, false), &consts),
            Some((Op::Mul, Operand::Reg(0), Operand::Imm(100_000)))
        );
        assert_eq!(
            int_arith(Inst::iabc(Op::AddK, 1, 0, 1, false), &consts),
            None
        );
        assert_eq!(
            int_arith(Inst::iabc(Op::AddK, 1, 0, 2, false), &consts),
            None
        );
        assert_eq!(
            int_arith(Inst::iabc(Op::ShrI, 1, 0, enc(1), false), &consts),
            None
        );
        assert_eq!(
            int_compare(Inst::iabc(Op::GtI, 0, enc(-1), 0, true), &consts),
            Some((IntPredicate::SGT, Operand::Imm(-1)))
        );
        assert_eq!(
            int_compare(Inst::iabc(Op::LtI, 0, enc(2), 1, false), &consts),
            None
        );
        assert_eq!(
            int_compare(Inst::iabc(Op::EqK, 0, 0, 0, false), &consts),
            Some((IntPredicate::EQ, Operand::Imm(100_000)))
        );
        assert_eq!(
            int_compare(Inst::iabc(Op::EqK, 0, 1, 0, false), &consts),
            None
        );
        assert_eq!(
            operand_regs(Inst::iabc(Op::LtI, 3, enc(9), 0, false)),
            vec![3]
        );
        assert_eq!(operand_regs(Inst::iabc(Op::Lt, 3, 4, 0, false)), vec![3, 4]);
    }

    #[test]
    fn mod_by_a_constant_zero_is_refused() {
        let consts = [Value::Int(0), Value::Int(-3)];
        assert_eq!(
            int_arith(Inst::iabc(Op::ModK, 1, 0, 0, false), &consts),
            None
        );
        assert_eq!(
            int_arith(Inst::iabc(Op::ModK, 1, 0, 1, false), &consts),
            Some((Op::Mod, Operand::Reg(0), Operand::Imm(-3)))
        );
    }
}
