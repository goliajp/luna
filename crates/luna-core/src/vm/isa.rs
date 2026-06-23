//! Instruction set: u32 instructions with the PUC 5.5 field layout
//! (op 7 | A 8 | k 1 | B 8 | C 8, plus Bx/sBx/Ax/sJ variants). The opcode
//! set follows lopcodes.h (v5.5.0) with deliberate v1 trims recorded in the
//! P03 plan: no K-/immediate-arith variants and no MMBIN* (metamethod
//! fallback is handled inline by the Rust dispatch loop); they return in the
//! P10 ceiling pass if profiles ask for them.

/// Opcode kinds for the luna bytecode. Layout follows PUC `lopcodes.h`
/// (5.5.0); semantics may differ where noted in the dispatcher.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Op {
    /// `R[A] := R[B]` register move.
    Move,
    /// `R[A] := sBx` load immediate integer.
    LoadI,
    /// `R[A] := (lua_Number)sBx` load immediate float.
    LoadF,
    /// `R[A] := K[Bx]` load constant.
    LoadK,
    /// `R[A] := K[extra_arg]` load constant with extended index (next op
    /// must be `ExtraArg`).
    LoadKx,
    /// `R[A] := false`.
    LoadFalse,
    /// `R[A] := false; pc++` load false and skip next instruction.
    LFalseSkip,
    /// `R[A] := true`.
    LoadTrue,
    /// `R[A..A+B] := nil` clear a register range.
    LoadNil,
    /// `R[A] := Upvalues[B]`.
    GetUpval,
    /// `Upvalues[B] := R[A]`.
    SetUpval,
    /// `R[A] := Upvalues[B][K[C]:string]` global-style table read on an
    /// upvalue.
    GetTabUp,
    /// `R[A] := R[B][R[C]]`.
    GetTable,
    /// `R[A] := R[B][C:int]` integer-indexed read.
    GetI,
    /// `R[A] := R[B][K[C]:string]` field read with constant key.
    GetField,
    /// `Upvalues[A][K[B]:string] := R[C]/K[C]`.
    SetTabUp,
    /// `R[A][R[B]] := R[C]/K[C]`.
    SetTable,
    /// `R[A][B:int] := R[C]/K[C]` integer-indexed write.
    SetI,
    /// `R[A][K[B]:string] := R[C]/K[C]` field write.
    SetField,
    /// `R[A] := {}` allocate a new table; B/C carry size hints.
    NewTable,
    /// `R[A+1] := R[B]; R[A] := R[B][K[C]:string]` self-method prep for
    /// `obj:m(...)`.
    SelfOp,
    /// `R[A] := R[B] + R[C]/K[C]`.
    Add,
    /// `R[A] := R[B] - R[C]/K[C]`.
    Sub,
    /// `R[A] := R[B] * R[C]/K[C]`.
    Mul,
    /// `R[A] := R[B] % R[C]/K[C]`.
    Mod,
    /// `R[A] := R[B] ^ R[C]/K[C]`.
    Pow,
    /// `R[A] := R[B] / R[C]/K[C]`.
    Div,
    /// `R[A] := R[B] // R[C]/K[C]`.
    IDiv,
    /// `R[A] := R[B] & R[C]/K[C]`.
    BAnd,
    /// `R[A] := R[B] | R[C]/K[C]`.
    BOr,
    /// `R[A] := R[B] ~ R[C]/K[C]`.
    BXor,
    /// `R[A] := R[B] << R[C]/K[C]`.
    Shl,
    /// `R[A] := R[B] >> R[C]/K[C]`.
    Shr,
    /// `R[A] := -R[B]` arithmetic negation.
    Unm,
    /// `R[A] := ~R[B]` bitwise NOT.
    BNot,
    /// `R[A] := not R[B]`.
    Not,
    /// `R[A] := #R[B]` length operator.
    Len,
    /// `R[A] := R[A] .. ... .. R[A+B-1]` string concatenation chain.
    Concat,
    /// Close upvalues in scope `A` (closes pending `<close>` and upvalues).
    Close,
    /// Mark to-be-closed slot `A` (5.4).
    Tbc,
    /// `pc += sJ` unconditional jump.
    Jmp,
    /// Equality comparison with optional skip.
    Eq,
    /// Less-than comparison with optional skip.
    Lt,
    /// Less-or-equal comparison with optional skip.
    Le,
    /// Equality against a constant.
    EqK,
    /// `if (not R[A]) == k then pc++`.
    Test,
    /// `if (not R[B]) == k then pc++ else R[A] := R[B]`.
    TestSet,
    /// `R[A], ..., R[A+C-2] := R[A](R[A+1], ..., R[A+B-1])`.
    Call,
    /// Tail call (same register/return contract as `Call`).
    TailCall,
    /// `return R[A], ..., R[A+B-2]`.
    Return,
    /// `return` with no values.
    Return0,
    /// `return R[A]` single-value return.
    Return1,
    /// Numeric-for iteration step.
    ForLoop,
    /// Numeric-for prepare (validates types, normalizes step).
    ForPrep,
    /// Generic-for prepare.
    TForPrep,
    /// Generic-for call: invoke iterator once.
    TForCall,
    /// Generic-for loop tail (branch back if iterator returned non-nil).
    TForLoop,
    /// Bulk-store a sequence into a table (table constructor).
    SetList,
    /// `R[A] := closure(KPROTO[Bx])`.
    Closure,
    /// `R[A], R[A+1], ..., R[A+C-2] := vararg`.
    Vararg,
    /// 5.5: materialize the vararg table into `R[A]` (named vararg that is
    /// written / escapes / is `_ENV`). Builds it from the stack varargs.
    GetVarg,
    /// 5.5: `R[A] := vararg[R[C]]` — index the *virtual* named vararg without
    /// allocating a table. Integer key in `[1,n]` → that vararg, key `"n"` →
    /// the count, else nil (PUC OP_GETVARG on an unmaterialized vararg).
    VargIdx,
    /// 5.5: error if `R[A]` is not nil — a defining `global` write whose target
    /// already exists. Bx is the name constant index + 1 (0 ⇒ unknown name).
    ErrNNil,
    /// Extended-immediate payload for the preceding instruction (see
    /// `LoadKx`).
    ExtraArg,
}

/// Total number of opcodes defined in [`Op`].
pub const NUM_OPS: usize = Op::ExtraArg as usize + 1;

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

impl Inst {
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
        debug_assert!((-MAX_SBX..=MAX_SBX).contains(&sbx));
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
