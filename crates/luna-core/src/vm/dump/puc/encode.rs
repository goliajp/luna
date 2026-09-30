//! Checked instruction encoders for translated PUC code.

use crate::vm::isa::{Inst, Op};

// A translated chunk's operands come from the input: an instruction is
// built only when every field fits luna's encoding. `Inst::iabc` and
// friends only debug-assert that, and in a release build an oversized
// operand spills into the neighbouring field.

/// [`Inst::iabc`], refusing operands that do not fit.
pub(super) fn enc_abc(op: Op, a: u32, b: u32, c: u32, k: bool) -> Result<Inst, String> {
    Inst::try_iabc(op, a, b, c, k)
        .ok_or_else(|| format!("{op:?} operands A={a} B={b} C={c} do not fit an instruction"))
}

/// [`Inst::iabx`], refusing operands that do not fit.
pub(super) fn enc_abx(op: Op, a: u32, bx: u32) -> Result<Inst, String> {
    Inst::try_iabx(op, a, bx)
        .ok_or_else(|| format!("{op:?} operands A={a} Bx={bx} do not fit an instruction"))
}

/// [`Inst::iasbx`], refusing operands that do not fit.
pub(super) fn enc_asbx(op: Op, a: u32, sbx: i32) -> Result<Inst, String> {
    Inst::try_iasbx(op, a, sbx)
        .ok_or_else(|| format!("{op:?} operands A={a} sBx={sbx} do not fit an instruction"))
}

/// [`Inst::iax`], refusing an operand that does not fit.
pub(super) fn enc_ax(op: Op, ax: u32) -> Result<Inst, String> {
    Inst::try_iax(op, ax)
        .ok_or_else(|| format!("{op:?} operand Ax={ax} does not fit an instruction"))
}

/// [`Inst::isj`], refusing a jump that does not fit.
pub(super) fn enc_sj(op: Op, sj: i32) -> Result<Inst, String> {
    Inst::try_isj(op, sj).ok_or_else(|| format!("{op:?} jump sJ={sj} does not fit an instruction"))
}

/// The "is a constant" bit of a PUC 5.1–5.3 RK operand.
pub(super) const RK_BIT: u32 = 1 << 8;
