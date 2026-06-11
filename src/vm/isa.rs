//! Instruction set: u32 instructions with the PUC 5.5 field layout
//! (op 7 | A 8 | k 1 | B 8 | C 8, plus Bx/sBx/Ax/sJ variants). The opcode
//! set follows lopcodes.h (v5.5.0) with deliberate v1 trims recorded in the
//! P03 plan: no K-/immediate-arith variants and no MMBIN* (metamethod
//! fallback is handled inline by the Rust dispatch loop); they return in the
//! P10 ceiling pass if profiles ask for them.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Op {
    Move,
    LoadI,
    LoadF,
    LoadK,
    LoadKx,
    LoadFalse,
    LFalseSkip,
    LoadTrue,
    LoadNil,
    GetUpval,
    SetUpval,
    GetTabUp,
    GetTable,
    GetI,
    GetField,
    SetTabUp,
    SetTable,
    SetI,
    SetField,
    NewTable,
    SelfOp,
    Add,
    Sub,
    Mul,
    Mod,
    Pow,
    Div,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    Unm,
    BNot,
    Not,
    Len,
    Concat,
    Close,
    Tbc,
    Jmp,
    Eq,
    Lt,
    Le,
    EqK,
    Test,
    TestSet,
    Call,
    TailCall,
    Return,
    Return0,
    Return1,
    ForLoop,
    ForPrep,
    TForPrep,
    TForCall,
    TForLoop,
    SetList,
    Closure,
    Vararg,
    /// 5.5: load the vararg table itself (named vararg binding)
    GetVarg,
    ExtraArg,
}

pub const NUM_OPS: usize = Op::ExtraArg as usize + 1;

/// One encoded instruction.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Inst(pub u32);

const POS_A: u32 = 7;
const POS_K: u32 = 15;
const POS_B: u32 = 16;
const POS_C: u32 = 24;
const POS_BX: u32 = 15;

pub const MAX_A: u32 = 0xFF;
pub const MAX_B: u32 = 0xFF;
pub const MAX_C: u32 = 0xFF;
pub const MAX_BX: u32 = (1 << 17) - 1;
pub const MAX_SBX: i32 = (MAX_BX >> 1) as i32; // 65535
pub const MAX_AX: u32 = (1 << 25) - 1;
pub const MAX_SJ: i32 = ((1u32 << 24) - 1) as i32; // sJ stored with this offset

impl Inst {
    pub fn iabc(op: Op, a: u32, b: u32, c: u32, k: bool) -> Inst {
        debug_assert!(a <= MAX_A && b <= MAX_B && c <= MAX_C);
        Inst(op as u32 | (a << POS_A) | ((k as u32) << POS_K) | (b << POS_B) | (c << POS_C))
    }

    pub fn iabx(op: Op, a: u32, bx: u32) -> Inst {
        debug_assert!(a <= MAX_A && bx <= MAX_BX);
        Inst(op as u32 | (a << POS_A) | (bx << POS_BX))
    }

    pub fn iasbx(op: Op, a: u32, sbx: i32) -> Inst {
        debug_assert!((-MAX_SBX..=MAX_SBX).contains(&sbx));
        Inst::iabx(op, a, (sbx + MAX_SBX) as u32)
    }

    pub fn iax(op: Op, ax: u32) -> Inst {
        debug_assert!(ax <= MAX_AX);
        Inst(op as u32 | (ax << POS_A))
    }

    pub fn isj(op: Op, sj: i32) -> Inst {
        debug_assert!((-MAX_SJ..=MAX_SJ).contains(&sj));
        Inst::iax(op, (sj + MAX_SJ) as u32)
    }

    #[inline(always)]
    pub fn op(self) -> Op {
        let raw = (self.0 & 0x7F) as u8;
        debug_assert!((raw as usize) < NUM_OPS, "corrupt opcode {raw}");
        // SAFETY: instructions are only built via the constructors above with
        // a valid Op; Op is repr(u8) and dense from 0..NUM_OPS.
        unsafe { std::mem::transmute::<u8, Op>(raw) }
    }

    #[inline(always)]
    pub fn a(self) -> u32 {
        (self.0 >> POS_A) & 0xFF
    }

    #[inline(always)]
    pub fn k(self) -> bool {
        (self.0 >> POS_K) & 1 != 0
    }

    #[inline(always)]
    pub fn b(self) -> u32 {
        (self.0 >> POS_B) & 0xFF
    }

    #[inline(always)]
    pub fn c(self) -> u32 {
        self.0 >> POS_C
    }

    #[inline(always)]
    pub fn bx(self) -> u32 {
        self.0 >> POS_BX
    }

    #[inline(always)]
    pub fn sbx(self) -> i32 {
        self.bx() as i32 - MAX_SBX
    }

    #[inline(always)]
    pub fn ax(self) -> u32 {
        self.0 >> POS_A
    }

    #[inline(always)]
    pub fn sj(self) -> i32 {
        self.ax() as i32 - MAX_SJ
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
