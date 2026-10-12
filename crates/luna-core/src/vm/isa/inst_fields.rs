//! Field helpers of particular instructions: jumps, and the operands of
//! `Concat`.

use super::Op;
use super::inst::{Inst, POS_K};

impl Inst {
    /// The offset of a jump (`Jmp`, `JmpClose`, `JmpCloseBack`): its
    /// target is `pc + 1 + offset`.
    pub fn jump_offset(self) -> i32 {
        match self.op() {
            Op::JmpClose => self.bx() as i32,
            Op::JmpCloseBack => -1 - self.bx() as i32,
            _ => self.sj(),
        }
    }

    /// This jump with its offset set to `off`; a closing jump changes
    /// between `JmpClose` and `JmpCloseBack` with the direction.
    pub(crate) fn with_jump_offset(self, off: i32) -> Inst {
        match self.op() {
            Op::JmpClose | Op::JmpCloseBack => Inst::jmp_close(self.a(), off),
            _ => {
                let mut i = self;
                i.set_sj(off);
                i
            }
        }
    }

    /// A 5.2 / 5.3 jump closing the upvalues from `R[a - 1]` on.
    pub(crate) fn jmp_close(a: u32, off: i32) -> Inst {
        if off >= 0 {
            Inst::iabx(Op::JmpClose, a, off as u32)
        } else {
            Inst::iabx(Op::JmpCloseBack, a, (-1 - off) as u32)
        }
    }

    /// This jump closing the upvalues from `R[a - 1]` on (PUC 5.2 / 5.3
    /// `luaK_patchclose`).
    pub(crate) fn with_close(self, a: u32) -> Inst {
        Inst::jmp_close(a, self.jump_offset())
    }

    /// A `Concat`'s first operand and its destination.
    pub fn concat_operands(self) -> (u32, u32) {
        if self.k() {
            (self.c(), self.a())
        } else {
            (self.a(), self.a())
        }
    }

    /// This instruction with its `k` flag set to `k`.
    pub(crate) fn with_k(self, k: bool) -> Inst {
        Inst((self.0 & !(1 << POS_K)) | ((k as u32) << POS_K))
    }
}
