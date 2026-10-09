//! What the code reading instructions asks of an opcode.

use super::Op;

/// Where a `for` loop keeps its control values: each dialect's parser lays
/// its loops out differently, and luna's loop opcodes follow the layout of
/// the dialect that compiled the code (`R[A]` is the loop's base).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ForLayout {
    /// numeric, 5.1–5.4: index, limit (5.4: count), step, then the variable
    Num,
    /// numeric, 5.5: count (or limit), step, then the variable, which is
    /// the index itself
    Num55,
    /// generic, 5.1–5.3: iterator, state, control, then the variables
    Gen53,
    /// generic, 5.4: iterator, state, control, closing value, then the
    /// variables
    Gen54,
    /// generic, 5.5: iterator, state, closing value, then the variables,
    /// the first of which is the control
    Gen55,
}

impl ForLayout {
    /// The register of the first loop variable, from the base.
    pub fn var(self) -> u32 {
        match self {
            ForLayout::Num | ForLayout::Gen53 | ForLayout::Gen55 => 3,
            ForLayout::Num55 => 2,
            ForLayout::Gen54 => 4,
        }
    }

    /// A generic loop's control value, passed to the iterator.
    pub fn control(self) -> u32 {
        if self == ForLayout::Gen55 { 3 } else { 2 }
    }

    /// A generic loop's closing value.
    pub fn closing(self) -> Option<u32> {
        match self {
            ForLayout::Gen54 => Some(3),
            ForLayout::Gen55 => Some(2),
            _ => None,
        }
    }

    /// Whether `TForLoop` copies the first variable into the control
    /// register (in 5.5 the variable is the control).
    pub fn copies_control(self) -> bool {
        matches!(self, ForLayout::Gen53 | ForLayout::Gen54)
    }

    /// The registers from the base that the loop's call uses: the iterator,
    /// state and control are copied to the first variable's register and
    /// the two after it, where the call runs.
    pub fn call_end(self) -> u32 {
        self.var() + 3
    }

    /// The variable count of a generic `TForCall` with its layout's
    /// first-variable and control registers, as one argument of the trace
    /// JIT's `TForCall` helper ([`ForLayout::unpack_call`]).
    pub fn pack_call(self, nvars: u32) -> i32 {
        (nvars | self.var() << 8 | self.control() << 12) as i32
    }

    /// `(nvars, first variable, control)` of [`ForLayout::pack_call`].
    pub fn unpack_call(packed: i32) -> (u32, u32, u32) {
        let p = packed as u32;
        (p & 0xFF, (p >> 8) & 0xF, (p >> 12) & 0xF)
    }

    /// The prepare, call and loop opcodes of a generic layout (`Op::Jmp`
    /// for the call of a numeric one, which has none).
    pub fn ops(self) -> (Op, Op, Op) {
        match self {
            ForLayout::Num => (Op::ForPrep, Op::Jmp, Op::ForLoop),
            ForLayout::Num55 => (Op::ForPrep55, Op::Jmp, Op::ForLoop55),
            ForLayout::Gen53 => (Op::TForPrep53, Op::TForCall53, Op::TForLoop53),
            ForLayout::Gen54 => (Op::TForPrep, Op::TForCall, Op::TForLoop),
            ForLayout::Gen55 => (Op::TForPrep55, Op::TForCall55, Op::TForLoop55),
        }
    }
}

impl Op {
    /// The layout of the loop a `for` opcode belongs to.
    pub fn for_layout(self) -> Option<ForLayout> {
        Some(match self {
            Op::ForPrep | Op::ForLoop => ForLayout::Num,
            Op::ForPrep55 | Op::ForLoop55 => ForLayout::Num55,
            Op::TForPrep53 | Op::TForCall53 | Op::TForLoop53 => ForLayout::Gen53,
            Op::TForPrep | Op::TForCall | Op::TForLoop => ForLayout::Gen54,
            Op::TForPrep55 | Op::TForCall55 | Op::TForLoop55 => ForLayout::Gen55,
            _ => return None,
        })
    }

    /// A numeric `for` prepare.
    pub fn is_for_prep(self) -> bool {
        matches!(self, Op::ForPrep | Op::ForPrep55)
    }

    /// A numeric `for` step.
    pub fn is_for_loop(self) -> bool {
        matches!(self, Op::ForLoop | Op::ForLoop55)
    }

    /// A generic `for` prepare.
    pub fn is_tfor_prep(self) -> bool {
        matches!(self, Op::TForPrep | Op::TForPrep53 | Op::TForPrep55)
    }

    /// A generic `for` call.
    pub fn is_tfor_call(self) -> bool {
        matches!(self, Op::TForCall | Op::TForCall53 | Op::TForCall55)
    }

    /// A generic `for` loop tail.
    pub fn is_tfor_loop(self) -> bool {
        matches!(self, Op::TForLoop | Op::TForLoop53 | Op::TForLoop55)
    }

    /// The register-operand opcode a constant- or immediate-operand
    /// arithmetic opcode computes, `None` for any other opcode (the
    /// two-constant forms are [`Op::arith_kk_op`]).
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
            Op::ShrI | Op::ShrK => Op::Shr,
            Op::ShlI | Op::ShlK => Op::Shl,
            _ => return None,
        })
    }

    /// The register-operand opcode a two-constant arithmetic opcode
    /// computes.
    pub fn arith_kk_op(self) -> Option<Op> {
        Some(match self {
            Op::AddKK => Op::Add,
            Op::SubKK => Op::Sub,
            Op::MulKK => Op::Mul,
            Op::ModKK => Op::Mod,
            Op::PowKK => Op::Pow,
            Op::DivKK => Op::Div,
            Op::IDivKK => Op::IDiv,
            Op::BAndKK => Op::BAnd,
            Op::BOrKK => Op::BOr,
            Op::BXorKK => Op::BXor,
            Op::ShlKK => Op::Shl,
            Op::ShrKK => Op::Shr,
            _ => return None,
        })
    }

    /// The two-constant form of a register-operand arithmetic opcode.
    pub fn kk_form(self) -> Option<Op> {
        Some(match self {
            Op::Add => Op::AddKK,
            Op::Sub => Op::SubKK,
            Op::Mul => Op::MulKK,
            Op::Mod => Op::ModKK,
            Op::Pow => Op::PowKK,
            Op::Div => Op::DivKK,
            Op::IDiv => Op::IDivKK,
            Op::BAnd => Op::BAndKK,
            Op::BOr => Op::BOrKK,
            Op::BXor => Op::BXorKK,
            Op::Shl => Op::ShlKK,
            Op::Shr => Op::ShrKK,
            _ => return None,
        })
    }

    /// The constant-operand form of a register-operand arithmetic opcode.
    pub fn k_form(self) -> Option<Op> {
        Some(match self {
            Op::Add => Op::AddK,
            Op::Sub => Op::SubK,
            Op::Mul => Op::MulK,
            Op::Mod => Op::ModK,
            Op::Pow => Op::PowK,
            Op::Div => Op::DivK,
            Op::IDiv => Op::IDivK,
            Op::BAnd => Op::BAndK,
            Op::BOr => Op::BOrK,
            Op::BXor => Op::BXorK,
            Op::Shl => Op::ShlK,
            Op::Shr => Op::ShrK,
            _ => return None,
        })
    }

    /// An unconditional jump: `Jmp`, or a 5.2 / 5.3 jump that closes.
    pub fn is_jump(self) -> bool {
        matches!(self, Op::Jmp | Op::JmpClose | Op::JmpCloseBack)
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
                | Op::LtK
                | Op::LeK
                | Op::EqKK
                | Op::LtKK
                | Op::LeKK
                | Op::Test
                | Op::TestSet
        )
    }
}
