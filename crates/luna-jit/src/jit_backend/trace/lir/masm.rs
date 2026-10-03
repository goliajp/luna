//! The instructions code generation needs from each target.

use super::alloc::Class;
use super::*;

/// An integer condition, after `cmp a, b`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cond {
    Eq,
    Ne,
    Slt,
    Sle,
    Sgt,
    Sge,
    Ult,
    Ule,
    Ugt,
    Uge,
}

impl Cond {
    pub(crate) fn of(cc: IntCC) -> Option<Cond> {
        Some(match cc {
            IntCC::Equal => Cond::Eq,
            IntCC::NotEqual => Cond::Ne,
            IntCC::SignedLessThan => Cond::Slt,
            IntCC::SignedLessThanOrEqual => Cond::Sle,
            IntCC::SignedGreaterThan => Cond::Sgt,
            IntCC::SignedGreaterThanOrEqual => Cond::Sge,
            IntCC::UnsignedLessThan => Cond::Ult,
            IntCC::UnsignedLessThanOrEqual => Cond::Ule,
            IntCC::UnsignedGreaterThan => Cond::Ugt,
            IntCC::UnsignedGreaterThanOrEqual => Cond::Uge,
        })
    }
    pub(crate) fn invert(self) -> Cond {
        match self {
            Cond::Eq => Cond::Ne,
            Cond::Ne => Cond::Eq,
            Cond::Slt => Cond::Sge,
            Cond::Sle => Cond::Sgt,
            Cond::Sgt => Cond::Sle,
            Cond::Sge => Cond::Slt,
            Cond::Ult => Cond::Uge,
            Cond::Ule => Cond::Ugt,
            Cond::Ugt => Cond::Ule,
            Cond::Uge => Cond::Ult,
        }
    }
    pub(crate) fn signed(self) -> bool {
        matches!(self, Cond::Slt | Cond::Sle | Cond::Sgt | Cond::Sge)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Alu {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    Shl,
    Lshr,
    Ashr,
    Sdiv,
    Udiv,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Width {
    B1,
    B2,
    B4,
    B8,
}

impl Width {
    pub(crate) fn of(t: Ty) -> Width {
        match t {
            Ty::I8 => Width::B1,
            Ty::I16 => Width::B2,
            Ty::I32 => Width::B4,
            Ty::I64 | Ty::F64 => Width::B8,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Label(pub(crate) u32);

/// The instructions code generation needs from a target. Registers are the
/// target's encoding numbers; integer and floating-point registers are
/// separate spaces. 32-bit (`wide == false`) results are zero-extended.
/// The buffers an assembler writes into, kept from one trace to the next.
#[derive(Default)]
pub(crate) struct Bufs {
    /// The finished code (x86-64 also assembles into it).
    pub(crate) bytes: Vec<u8>,
    /// AArch64 instruction words.
    pub(crate) words: Vec<u32>,
    pub(crate) labels: Vec<u32>,
    /// (code offset, label, kind)
    pub(crate) fixups: Vec<(u32, u32, u8)>,
    /// Where the relocated addresses sit in `bytes`.
    pub(crate) sites: Vec<crate::jit_backend::trace::reloc::Site>,
}

pub(crate) trait Masm {
    /// The stack pointer as a base register.
    const SP: u8;
    const INT: Class;
    const FLT: Class;
    /// Never allocated: operand reloads and move cycles.
    const SCRATCH: [u8; 2];
    const FSCRATCH: [u8; 2];
    /// Holds an indirect call's target while the arguments are moved.
    const CALL_TARGET: u8;
    const INT_ARGS: &'static [u8];
    const FLOAT_ARGS: &'static [u8];
    const RET: u8;
    const FRET: u8;
    /// Bytes at the stack pointer a callee may use (Win64's shadow space).
    const CALL_SHADOW: u32;
    /// Argument `k` goes in integer or float register `k` (Win64), rather
    /// than in the next free register of its class.
    const POSITIONAL_ARGS: bool;

    fn new_label(&mut self) -> Label;
    fn bind(&mut self, l: Label);
    fn jmp(&mut self, l: Label);
    fn jcc(&mut self, c: Cond, l: Label);
    /// Branch to `l` when `r` is non-zero (`nz`) or zero.
    fn branch_reg(&mut self, r: u8, nz: bool, l: Label);

    fn mov(&mut self, d: u8, s: u8);
    fn mov_imm(&mut self, d: u8, v: i64);
    /// `d = v` in a form of fixed length, noting it as relocation `n`.
    fn mov_reloc(&mut self, d: u8, v: i64, n: u32);
    fn fmov(&mut self, d: u8, s: u8);
    fn bits_to_f(&mut self, d: u8, s: u8);
    fn bits_to_i(&mut self, d: u8, s: u8);
    /// Zero-extending load.
    fn load(&mut self, w: Width, d: u8, base: u8, off: i32);
    fn store(&mut self, w: Width, s: u8, base: u8, off: i32);
    fn fload(&mut self, d: u8, base: u8, off: i32);
    fn fstore(&mut self, s: u8, base: u8, off: i32);
    fn lea(&mut self, d: u8, base: u8, off: i32);

    fn alu(&mut self, op: Alu, wide: bool, d: u8, a: u8, b: u8);
    /// `false` when `imm` has no encoding for `op` (nothing emitted).
    fn alu_imm(&mut self, op: Alu, wide: bool, d: u8, a: u8, imm: i64) -> bool;
    fn neg(&mut self, wide: bool, d: u8, a: u8);
    fn not(&mut self, wide: bool, d: u8, a: u8);
    fn cmp(&mut self, wide: bool, a: u8, b: u8);
    fn cmp_imm(&mut self, wide: bool, a: u8, imm: i64) -> bool;
    fn setcc(&mut self, d: u8, c: Cond);
    /// `d = c ? a : b`
    fn csel(&mut self, d: u8, c: Cond, a: u8, b: u8);
    fn zext(&mut self, d: u8, a: u8, bits: u32);
    fn sext(&mut self, d: u8, a: u8, bits: u32);

    fn fbin(&mut self, op: BinOp, d: u8, a: u8, b: u8);
    fn fneg(&mut self, d: u8, a: u8);
    /// `false` when the target cannot round in place (nothing emitted).
    fn fround(&mut self, d: u8, a: u8, up: bool) -> bool;
    fn i2f(&mut self, d: u8, a: u8);
    fn f2i(&mut self, d: u8, a: u8);
    /// NaN to 0, out of range to the nearest bound.
    fn f2i_sat(&mut self, d: u8, a: u8);
    fn fcmp_set(&mut self, d: u8, cc: FloatCC, a: u8, b: u8) -> bool;

    fn call_abs(&mut self, addr: usize);
    fn call_reg(&mut self, r: u8);
    /// Saves `saved` / `fsaved` and reserves `locals` bytes (a multiple of
    /// 16) at the stack pointer.
    fn prologue(&mut self, saved: &[u8], fsaved: &[u8], locals: u32);
    fn epilogue_ret(&mut self);
    fn new(b: Bufs) -> Self;
    /// The buffers back, `bytes` holding the code.
    fn finish(self) -> Bufs;
}
