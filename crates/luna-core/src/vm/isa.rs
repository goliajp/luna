//! Instruction set: u32 instructions with the PUC 5.5 field layout
//! (op 7 | A 8 | k 1 | B 8 | C 8, plus Bx/sBx/Ax/sJ variants). The opcode
//! set follows lopcodes.h (v5.5.0) with one deliberate trim: no MMBIN*
//! (metamethod fallback is handled inline by the Rust dispatch loop, so the
//! constant- and immediate-operand arithmetic opcodes name their own
//! operator).

mod op;
mod op_info;
pub use op::Op;
pub use op_info::ForLayout;

/// Total number of opcodes defined in [`Op`].
pub const NUM_OPS: usize = Op::LTrueSkip as usize + 1;

mod inst;
mod inst_fields;
pub use inst::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_fields() {
        let i = Inst::iabc(Op::GetField, 200, 17, 255, true);
        assert_eq!(i.op(), Op::GetField);
        assert_eq!(i.a(), 200);
        assert_eq!(i.b(), 17);
        assert_eq!(i.c(), 255);
        assert!(i.k());

        let j = Inst::iasbx(Op::LoadI, 3, -42);
        assert_eq!(j.op(), Op::LoadI);
        assert_eq!(j.a(), 3);
        assert_eq!(j.sbx(), -42);

        let mut k = Inst::isj(Op::Jmp, -1);
        assert_eq!(k.sj(), -1);
        k.set_sj(12345);
        assert_eq!(k.op(), Op::Jmp);
        assert_eq!(k.sj(), 12345);

        let x = Inst::iax(Op::ExtraArg, MAX_AX);
        assert_eq!(x.ax(), MAX_AX);
    }
}
