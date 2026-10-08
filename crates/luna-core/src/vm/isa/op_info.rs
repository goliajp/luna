//! What the code reading instructions asks of an opcode.

use super::Op;

impl Op {
    /// The register-operand opcode a constant- or immediate-operand
    /// arithmetic opcode computes, `None` for any other opcode.
    pub fn arith_const_op(self) -> Option<Op> {
        Some(match self {
            Op::AddI | Op::AddK => Op::Add,
            Op::SubI | Op::SubK => Op::Sub,
            Op::MulK => Op::Mul,
            Op::ModK => Op::Mod,
            Op::PowK => Op::Pow,
            Op::DivK => Op::Div,
            Op::IDivK => Op::IDiv,
            Op::BAndK => Op::BAnd,
            Op::BOrK => Op::BOr,
            Op::BXorK => Op::BXor,
            Op::ShrI => Op::Shr,
            Op::ShlI => Op::Shl,
            _ => return None,
        })
    }

    /// A conditional test: the instruction after it is the `Jmp` it may skip.
    pub fn is_test(self) -> bool {
        matches!(
            self,
            Op::Eq
                | Op::Lt
                | Op::Le
                | Op::EqK
                | Op::EqI
                | Op::LtI
                | Op::LeI
                | Op::GtI
                | Op::GeI
                | Op::Test
                | Op::TestSet
        )
    }
}
