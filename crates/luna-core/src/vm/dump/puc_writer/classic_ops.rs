//! 5.1's opcode numbers for the instruction kinds 5.1 shares with 5.2.

use crate::vm::dump::puc::classic::Kind;
use crate::vm::dump::puc::puc_51 as p51;
use crate::vm::isa::Op;

/// 5.1 opcode of `k`, for the kinds 5.1 shares with 5.2.
pub(super) fn op51(k: Kind) -> Option<u8> {
    Some(match k {
        Kind::Move => p51::OP_MOVE,
        Kind::LoadK => p51::OP_LOADK,
        Kind::LoadBool => p51::OP_LOADBOOL,
        Kind::LoadNil => p51::OP_LOADNIL,
        Kind::GetUpval => p51::OP_GETUPVAL,
        Kind::GetTable => p51::OP_GETTABLE,
        Kind::SetUpval => p51::OP_SETUPVAL,
        Kind::SetTable => p51::OP_SETTABLE,
        Kind::NewTable => p51::OP_NEWTABLE,
        Kind::SelfOp => p51::OP_SELF,
        Kind::Arith(Op::Add) => p51::OP_ADD,
        Kind::Arith(Op::Sub) => p51::OP_SUB,
        Kind::Arith(Op::Mul) => p51::OP_MUL,
        Kind::Arith(Op::Div) => p51::OP_DIV,
        Kind::Arith(Op::Mod) => p51::OP_MOD,
        Kind::Arith(Op::Pow) => p51::OP_POW,
        Kind::Unary(Op::Unm) => p51::OP_UNM,
        Kind::Unary(Op::Not) => p51::OP_NOT,
        Kind::Unary(Op::Len) => p51::OP_LEN,
        Kind::Concat => p51::OP_CONCAT,
        Kind::Jmp => p51::OP_JMP,
        Kind::Eq => p51::OP_EQ,
        Kind::Lt => p51::OP_LT,
        Kind::Le => p51::OP_LE,
        Kind::Test => p51::OP_TEST,
        Kind::TestSet => p51::OP_TESTSET,
        Kind::Call => p51::OP_CALL,
        Kind::TailCall => p51::OP_TAILCALL,
        Kind::Return => p51::OP_RETURN,
        Kind::ForLoop => p51::OP_FORLOOP,
        Kind::ForPrep => p51::OP_FORPREP,
        Kind::SetList => p51::OP_SETLIST,
        Kind::Closure => p51::OP_CLOSURE,
        Kind::Vararg => p51::OP_VARARG,
        _ => return None,
    })
}
