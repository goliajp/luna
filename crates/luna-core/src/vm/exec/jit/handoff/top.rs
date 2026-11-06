//! The stack top the interpreter runs an op with when compiled code has
//! handed the op's level over before it.

use crate::vm::isa::{Inst, Op};

/// Whether the interpreter takes `op`'s operands up to the stack top when
/// its `B` is 0: the values an open call or `VARARG` before it left.
pub fn takes_top(op: Op) -> bool {
    match op {
        Op::SetList | Op::Call | Op::TailCall | Op::Return => true,
        Op::Move
        | Op::LoadI
        | Op::LoadF
        | Op::LoadK
        | Op::LoadKx
        | Op::LoadFalse
        | Op::LFalseSkip
        | Op::LoadTrue
        | Op::LoadNil
        | Op::GetUpval
        | Op::SetUpval
        | Op::GetTabUp
        | Op::GetTable
        | Op::GetI
        | Op::GetField
        | Op::SetTabUp
        | Op::SetTable
        | Op::SetI
        | Op::SetField
        | Op::NewTable
        | Op::SelfOp
        | Op::Add
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
        | Op::Shr
        | Op::Unm
        | Op::BNot
        | Op::Not
        | Op::Len
        | Op::Concat
        | Op::Close
        | Op::Tbc
        | Op::Jmp
        | Op::Eq
        | Op::Lt
        | Op::Le
        | Op::EqK
        | Op::Test
        | Op::TestSet
        | Op::Return0
        | Op::Return1
        | Op::ForLoop
        | Op::ForPrep
        | Op::TForPrep
        | Op::TForCall
        | Op::TForLoop
        | Op::Closure
        | Op::Vararg
        | Op::GetVarg
        | Op::VargIdx
        | Op::ErrNNil
        | Op::ExtraArg
        | Op::AddI
        | Op::SubI
        | Op::AddK
        | Op::SubK
        | Op::MulK
        | Op::ModK
        | Op::PowK
        | Op::DivK
        | Op::IDivK
        | Op::BAndK
        | Op::BOrK
        | Op::BXorK
        | Op::ShrI
        | Op::ShlI
        | Op::EqI
        | Op::LtI
        | Op::LeI
        | Op::GtI
        | Op::GeI
        | Op::GetTableK
        | Op::SetTableK
        | Op::GetTabUpR
        | Op::SetTabUpR
        | Op::SetTabUpK
        | Op::ForPrep55
        | Op::ForLoop55
        | Op::TForPrep53
        | Op::TForCall53
        | Op::TForLoop53
        | Op::TForPrep55
        | Op::TForCall55
        | Op::TForLoop55
        | Op::ShlK
        | Op::ShrK
        | Op::AddKK
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
        | Op::ShrKK
        | Op::LtK
        | Op::LeK
        | Op::EqKK
        | Op::LtKK
        | Op::LeKK
        | Op::GetGlobal
        | Op::SetGlobal
        | Op::JmpClose
        | Op::JmpCloseBack
        | Op::LTrueSkip
        | Op::ForLoopF
        | Op::ForLoop55F => false,
    }
}

/// The ops whose open form compiled code may hand a level over at: an open
/// `SETLIST` right after an open call of the function itself, which
/// returns one value.
pub const OPEN_AFTER_CALL: &[Op] = &[Op::SetList];

/// The stack top, from the level's base, the interpreter runs the op at
/// `pc` of `code` with: an open op's, the end of the value the call before
/// it returned; any other op's, the frame's `max_stack`.
pub(crate) fn handoff_top(code: &[Inst], pc: u32, max_stack: u32) -> u32 {
    let op = code[pc as usize];
    if op.b() != 0 || !takes_top(op.op()) {
        return max_stack;
    }
    let call = code[pc as usize - 1];
    debug_assert!(
        OPEN_AFTER_CALL.contains(&op.op()) && call.op() == Op::Call && call.c() == 0,
        "compiled code handed over at an open {:?} it cannot be at",
        op.op()
    );
    call.a() + 1
}
