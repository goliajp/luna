//! The encoded instruction word and its fields.

use super::{NUM_OPS, Op};

/// One encoded instruction.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Inst(
    /// The 32-bit packed instruction word; layout depends on the
    /// instruction format (iABC / iABx / iAsBx / iAx / isJ).
    pub u32,
);

const POS_A: u32 = 7;
const POS_K: u32 = 15;
const POS_B: u32 = 16;
const POS_C: u32 = 24;
const POS_BX: u32 = 15;

/// Maximum value encodable in the `A` field.
pub const MAX_A: u32 = 0xFF;
/// Maximum value encodable in the `B` field.
pub const MAX_B: u32 = 0xFF;
/// Maximum value encodable in the `C` field.
pub const MAX_C: u32 = 0xFF;
/// Maximum value encodable in the `Bx` field.
pub const MAX_BX: u32 = (1 << 17) - 1;
/// Maximum value encodable in the signed `sBx` field (bias = MAX_BX/2).
pub const MAX_SBX: i32 = (MAX_BX >> 1) as i32; // 65535
/// Maximum value encodable in the `Ax` field.
pub const MAX_AX: u32 = (1 << 25) - 1;
/// Maximum magnitude encodable in the signed `sJ` (jump offset) field.
pub const MAX_SJ: i32 = ((1u32 << 24) - 1) as i32; // sJ stored with this offset

/// Bias of the signed `sB` / `sC` fields (PUC `OFFSET_sC`).
pub const OFFSET_SC: i32 = 127;
/// Smallest value encodable in `sB` / `sC`.
pub const MIN_SC: i32 = -OFFSET_SC;
/// Largest value encodable in `sB` / `sC`.
pub const MAX_SC: i32 = MAX_C as i32 - OFFSET_SC;

impl Inst {
    /// [`Inst::iabc`] for operands that come from outside (a translated
    /// PUC chunk): `None` when a field does not fit, instead of an encoding
    /// that spills into the neighbouring field.
    pub(crate) fn try_iabc(op: Op, a: u32, b: u32, c: u32, k: bool) -> Option<Inst> {
        (a <= MAX_A && b <= MAX_B && c <= MAX_C).then(|| Inst::iabc(op, a, b, c, k))
    }

    /// [`Inst::iabx`] with the range check of [`Inst::try_iabc`].
    pub(crate) fn try_iabx(op: Op, a: u32, bx: u32) -> Option<Inst> {
        (a <= MAX_A && bx <= MAX_BX).then(|| Inst::iabx(op, a, bx))
    }

    /// [`Inst::iasbx`] with the range check of [`Inst::try_iabc`].
    pub(crate) fn try_iasbx(op: Op, a: u32, sbx: i32) -> Option<Inst> {
        (a <= MAX_A && (-MAX_SBX..=MAX_BX as i32 - MAX_SBX).contains(&sbx))
            .then(|| Inst::iasbx(op, a, sbx))
    }

    /// [`Inst::iax`] with the range check of [`Inst::try_iabc`].
    pub(crate) fn try_iax(op: Op, ax: u32) -> Option<Inst> {
        (ax <= MAX_AX).then(|| Inst::iax(op, ax))
    }

    /// [`Inst::isj`] with the range check of [`Inst::try_iabc`].
    pub(crate) fn try_isj(op: Op, sj: i32) -> Option<Inst> {
        (-MAX_SJ..=MAX_SJ).contains(&sj).then(|| Inst::isj(op, sj))
    }

    /// Build an iABC-format instruction (`A`, `B`, `C`, `k` flag).
    pub fn iabc(op: Op, a: u32, b: u32, c: u32, k: bool) -> Inst {
        debug_assert!(a <= MAX_A && b <= MAX_B && c <= MAX_C);
        Inst(op as u32 | (a << POS_A) | ((k as u32) << POS_K) | (b << POS_B) | (c << POS_C))
    }

    /// Build an iABx-format instruction (`A`, unsigned `Bx`).
    pub fn iabx(op: Op, a: u32, bx: u32) -> Inst {
        debug_assert!(a <= MAX_A && bx <= MAX_BX);
        Inst(op as u32 | (a << POS_A) | (bx << POS_BX))
    }

    /// Build an iAsBx-format instruction (`A`, signed `sBx`).
    pub fn iasbx(op: Op, a: u32, sbx: i32) -> Inst {
        // Bx is biased by MAX_SBX, so the top value, MAX_SBX + 1, fits too
        // (PUC 5.4+ `LOADI` uses it)
        debug_assert!((-MAX_SBX..=MAX_BX as i32 - MAX_SBX).contains(&sbx));
        Inst::iabx(op, a, (sbx + MAX_SBX) as u32)
    }

    /// Build an iAx-format instruction (unsigned 25-bit `Ax`).
    pub fn iax(op: Op, ax: u32) -> Inst {
        debug_assert!(ax <= MAX_AX);
        Inst(op as u32 | (ax << POS_A))
    }

    /// Build an isJ-format instruction (signed jump offset `sJ`).
    pub fn isj(op: Op, sj: i32) -> Inst {
        debug_assert!((-MAX_SJ..=MAX_SJ).contains(&sj));
        Inst::iax(op, (sj + MAX_SJ) as u32)
    }

    /// Decode the opcode field.
    #[inline(always)]
    pub fn op(self) -> Op {
        let raw = (self.0 & 0x7F) as u8;
        debug_assert!((raw as usize) < NUM_OPS, "corrupt opcode {raw}");
        // SAFETY: instructions are only built via the constructors above with
        // a valid Op; Op is repr(u8) and dense from 0..NUM_OPS.
        unsafe { std::mem::transmute::<u8, Op>(raw) }
    }

    /// Decode the `A` field.
    #[inline(always)]
    pub fn a(self) -> u32 {
        (self.0 >> POS_A) & 0xFF
    }

    /// Decode the `k` flag (constant-vs-register selector for some ops).
    #[inline(always)]
    pub fn k(self) -> bool {
        (self.0 >> POS_K) & 1 != 0
    }

    /// The operator an instruction stands for in the source, as its
    /// register-operand opcode: an `Add` with `k` set is a subtraction (see
    /// [`Op::Add`]), and a constant- or immediate-operand opcode is the
    /// operator it abbreviates.
    pub(crate) fn source_op(self) -> Op {
        match self.op() {
            Op::LtI | Op::GtI | Op::LtK | Op::LtKK => Op::Lt,
            Op::LeI | Op::GeI | Op::LeK | Op::LeKK => Op::Le,
            Op::EqKK => Op::EqK,
            op => op.arith_const_op().or(op.arith_kk_op()).unwrap_or(op),
        }
    }

    /// Decode the `B` field.
    #[inline(always)]
    pub fn b(self) -> u32 {
        (self.0 >> POS_B) & 0xFF
    }

    /// Decode the `C` field.
    #[inline(always)]
    pub fn c(self) -> u32 {
        self.0 >> POS_C
    }

    /// Decode the signed `sB` field.
    #[inline(always)]
    pub fn sb(self) -> i32 {
        self.b() as i32 - OFFSET_SC
    }

    /// Decode the signed `sC` field.
    #[inline(always)]
    pub fn sc(self) -> i32 {
        self.c() as i32 - OFFSET_SC
    }

    /// [`Op::arith_const_op`] of this instruction's opcode.
    pub fn arith_const_op(self) -> Option<Op> {
        self.op().arith_const_op()
    }

    /// [`Op::arith_kk_op`] of this instruction's opcode.
    pub fn arith_kk_op(self) -> Option<Op> {
        self.op().arith_kk_op()
    }

    /// A constant- or immediate-operand instruction as the two instructions
    /// it abbreviates: a load of the operand into register `scratch`, then
    /// the register-operand opcode. `None` for any other instruction.
    pub fn split_const_operand(self, scratch: u32) -> Option<[Inst; 2]> {
        let (a, b) = (self.a(), self.b());
        if let Some(op) = self.arith_const_op() {
            let load = match self.op() {
                // `SubI` numbers compute as PUC's `ADDI` with the negated
                // immediate (see `Op::SubI`)
                Op::SubI => {
                    return Some([
                        Inst::iasbx(Op::LoadI, scratch, -self.sc()),
                        Inst::iabc(Op::Add, a, b, scratch, false),
                    ]);
                }
                Op::AddI | Op::ShrI | Op::ShlI => Inst::iasbx(Op::LoadI, scratch, self.sc()),
                _ => Inst::iabx(Op::LoadK, scratch, self.c()),
            };
            let (l, r) = if self.k() { (scratch, b) } else { (b, scratch) };
            return Some([load, Inst::iabc(op, a, l, r, false)]);
        }
        let (op, swap) = match self.op() {
            Op::EqI => (Op::Eq, false),
            Op::LtI => (Op::Lt, false),
            Op::LeI => (Op::Le, false),
            Op::GtI => (Op::Lt, true),
            Op::GeI => (Op::Le, true),
            _ => return None,
        };
        let load = if self.c() != 0 { Op::LoadF } else { Op::LoadI };
        let (l, r) = if swap { (scratch, a) } else { (a, scratch) };
        Some([
            Inst::iasbx(load, scratch, self.sb()),
            Inst::iabc(op, l, r, 0, self.k()),
        ])
    }

    /// Decode the unsigned `Bx` field.
    #[inline(always)]
    pub fn bx(self) -> u32 {
        self.0 >> POS_BX
    }

    /// Decode the signed `sBx` field.
    #[inline(always)]
    pub fn sbx(self) -> i32 {
        self.bx() as i32 - MAX_SBX
    }

    /// Decode the unsigned `Ax` field.
    #[inline(always)]
    pub fn ax(self) -> u32 {
        self.0 >> POS_A
    }

    /// Decode the signed jump offset `sJ`.
    #[inline(always)]
    pub fn sj(self) -> i32 {
        self.ax() as i32 - MAX_SJ
    }

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

    /// Patch the sJ field of a jump (forward-jump backfill).
    pub fn set_sj(&mut self, sj: i32) {
        debug_assert!((-MAX_SJ..=MAX_SJ).contains(&sj));
        self.0 = (self.0 & 0x7F) | (((sj + MAX_SJ) as u32) << POS_A);
    }
}

impl std::fmt::Debug for Inst {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} a={} b={} c={} k={}",
            self.op(),
            self.a(),
            self.b(),
            self.c(),
            self.k()
        )
    }
}
