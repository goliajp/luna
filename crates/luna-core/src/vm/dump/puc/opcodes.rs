//! PUC opcodes by instruction kind, for the encoder: each dialect's
//! table of kinds (`OPS`) turned around, built at compile time.

use super::{classic, modern};

/// The opcode of a kind a dialect has none for.
pub(in crate::vm::dump) const NO_OP: u8 = u8::MAX;

/// Slots: the kinds without an operator first, then `Arith` and `Unary`
/// by their operator.
const SLOTS: usize = 640;

/// Each kind's opcode in a dialect's table of kinds (its first place).
pub(in crate::vm::dump) struct Opcodes([u8; SLOTS]);

macro_rules! opcodes {
    ($name:ident, $kind:ty, $slot:ident) => {
        impl Opcodes {
            pub(in crate::vm::dump) const fn $name(ops: &[$kind]) -> Opcodes {
                let mut t = [NO_OP; SLOTS];
                let mut i = ops.len();
                while i > 0 {
                    i -= 1;
                    t[$slot(ops[i])] = i as u8;
                }
                Opcodes(t)
            }
        }
    };
}
opcodes!(modern, modern::Kind, modern_slot);
opcodes!(classic, classic::Kind, classic_slot);

impl Opcodes {
    /// The opcode of modern kind `k`, if the dialect has one.
    pub(in crate::vm::dump) fn of_modern(&self, k: modern::Kind) -> Option<u32> {
        Some(self.0[modern_slot(k)])
            .filter(|&n| n != NO_OP)
            .map(u32::from)
    }

    /// The opcode of classic kind `k`, if the dialect has one.
    pub(in crate::vm::dump) fn of_classic(&self, k: classic::Kind) -> Option<u32> {
        Some(self.0[classic_slot(k)])
            .filter(|&n| n != NO_OP)
            .map(u32::from)
    }
}

const fn modern_slot(k: modern::Kind) -> usize {
    match k {
        modern::Kind::Move => 0,
        modern::Kind::LoadI => 1,
        modern::Kind::LoadF => 2,
        modern::Kind::LoadK => 3,
        modern::Kind::LoadKx => 4,
        modern::Kind::LoadFalse => 5,
        modern::Kind::LFalseSkip => 6,
        modern::Kind::LoadTrue => 7,
        modern::Kind::LoadNil => 8,
        modern::Kind::GetUpval => 9,
        modern::Kind::SetUpval => 10,
        modern::Kind::GetTabUp => 11,
        modern::Kind::GetTable => 12,
        modern::Kind::GetI => 13,
        modern::Kind::GetField => 14,
        modern::Kind::SetTabUp => 15,
        modern::Kind::SetTable => 16,
        modern::Kind::SetI => 17,
        modern::Kind::SetField => 18,
        modern::Kind::NewTable => 19,
        modern::Kind::SelfOp => 20,
        modern::Kind::ArithI => 21,
        modern::Kind::ArithK => 22,
        modern::Kind::Arith(op) => 128 + op as usize,
        modern::Kind::MmBin => 23,
        modern::Kind::MmBinI => 24,
        modern::Kind::MmBinK => 25,
        modern::Kind::Unary(op) => 384 + op as usize,
        modern::Kind::Concat => 26,
        modern::Kind::Close => 27,
        modern::Kind::Tbc => 28,
        modern::Kind::Jmp => 29,
        modern::Kind::Eq => 30,
        modern::Kind::Lt => 31,
        modern::Kind::Le => 32,
        modern::Kind::EqK => 33,
        modern::Kind::EqI => 34,
        modern::Kind::LtI => 35,
        modern::Kind::LeI => 36,
        modern::Kind::GtI => 37,
        modern::Kind::GeI => 38,
        modern::Kind::Test => 39,
        modern::Kind::TestSet => 40,
        modern::Kind::Call => 41,
        modern::Kind::TailCall => 42,
        modern::Kind::Return => 43,
        modern::Kind::Return0 => 44,
        modern::Kind::Return1 => 45,
        modern::Kind::ForLoop => 46,
        modern::Kind::ForPrep => 47,
        modern::Kind::TForPrep => 48,
        modern::Kind::TForCall => 49,
        modern::Kind::TForLoop => 50,
        modern::Kind::SetList => 51,
        modern::Kind::Closure => 52,
        modern::Kind::Vararg => 53,
        modern::Kind::GetVarg => 54,
        modern::Kind::ErrNNil => 55,
        modern::Kind::VarargPrep => 56,
        modern::Kind::ExtraArg => 57,
    }
}

const fn classic_slot(k: classic::Kind) -> usize {
    match k {
        classic::Kind::Move => 0,
        classic::Kind::LoadK => 1,
        classic::Kind::LoadKx => 2,
        classic::Kind::LoadBool => 3,
        classic::Kind::LoadNil => 4,
        classic::Kind::GetUpval => 5,
        classic::Kind::GetTabUp => 6,
        classic::Kind::GetTable => 7,
        classic::Kind::SetTabUp => 8,
        classic::Kind::SetUpval => 9,
        classic::Kind::SetTable => 10,
        classic::Kind::NewTable => 11,
        classic::Kind::SelfOp => 12,
        classic::Kind::Arith(op) => 128 + op as usize,
        classic::Kind::Unary(op) => 384 + op as usize,
        classic::Kind::Concat => 13,
        classic::Kind::Jmp => 14,
        classic::Kind::Eq => 15,
        classic::Kind::Lt => 16,
        classic::Kind::Le => 17,
        classic::Kind::Test => 18,
        classic::Kind::TestSet => 19,
        classic::Kind::Call => 20,
        classic::Kind::TailCall => 21,
        classic::Kind::Return => 22,
        classic::Kind::ForLoop => 23,
        classic::Kind::ForPrep => 24,
        classic::Kind::TForCall => 25,
        classic::Kind::TForLoop => 26,
        classic::Kind::SetList => 27,
        classic::Kind::Closure => 28,
        classic::Kind::Vararg => 29,
        classic::Kind::ExtraArg => 30,
    }
}

/// Each dialect's opcodes by kind.
pub(in crate::vm::dump) const OPCODES_52: Opcodes = Opcodes::classic(super::puc_52::OPS);
pub(in crate::vm::dump) const OPCODES_53: Opcodes = Opcodes::classic(super::puc_53::OPS);
pub(in crate::vm::dump) const OPCODES_54: Opcodes = Opcodes::modern(super::puc_54::OPS);
pub(in crate::vm::dump) const OPCODES_55: Opcodes = Opcodes::modern(super::puc_55::OPS);
