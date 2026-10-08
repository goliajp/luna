//! Code translation shared by PUC 5.4 and 5.5.
//!
//! luna's ISA descends from 5.4's layout (`op:7 | A:8 | k:1 | B:8 | C:8`),
//! so most opcodes map one to one. Semantics follow `lua-5.4.9/src/lvm.c`
//! and `lua-5.5.1/src/lvm.c`; what needs lowering:
//!
//! - **Immediate and constant arithmetic** (`ADDI`, `ADDK`…`BXORK`, `SHRI`,
//!   `SHLI`): the `MMBINI` / `MMBINK` that PUC always emits next records
//!   the metamethod event, the operand as written in the source, and
//!   whether it was the left one, so the pair becomes one luna op on the
//!   original operator and operand: `x - 1`, compiled as `ADDI x -1;
//!   MMBINI x 1 __sub`, is luna's `SubI x 1`. The `MMBIN*` then emits
//!   nothing (luna's arithmetic ops fall back to metamethods themselves).
//! - **`NEWTABLE`** always carries an `EXTRAARG`; luna's `NewTable` holds
//!   the hash size as is and the array size cut at 255, so both become one
//!   op.
//! - **`VARARGPREP`**: luna adjusts varargs at call time; in 5.5 it also
//!   sets the vararg parameter's register — a table (`PF_VATAB`, becomes
//!   `GetVarg`) or nil (`LoadNil`).
//!
//! 5.5 differs from 5.4 in its opcode numbering, in 6/10-bit `NEWTABLE` /
//! `SETLIST` operands, in `SELF` always taking a constant key, and in its
//! layout of both kinds of `for` loop (luna's `ForPrep55`, `TForCall55`...).

use super::lower::{Jump, Lowered, Lowering, RawProto, enc_abc, enc_abx, enc_asbx, enc_sj};
use crate::vm::isa::{self, ForLayout, Op};

mod ops;
use ops::{closure, compare, const_arith, for_jump, self_op, set_list, store, vararg_prep};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::vm::dump) enum Kind {
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
    /// `ADDI`, `SHRI`, `SHLI` — operator and operand come from the `MMBINI`.
    ArithI,
    /// `ADDK`…`BXORK` — operator and operand come from the `MMBINK`.
    ArithK,
    /// `R[A] := R[B] op R[C]`
    Arith(Op),
    MmBin,
    MmBinI,
    MmBinK,
    /// `R[A] := op R[B]`
    Unary(Op),
    Concat,
    Close,
    Tbc,
    Jmp,
    Eq,
    Lt,
    Le,
    EqK,
    EqI,
    LtI,
    LeI,
    GtI,
    GeI,
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
    GetVarg,
    ErrNNil,
    VarargPrep,
    ExtraArg,
}

/// What distinguishes the two dialects.
pub(super) struct Dialect {
    pub name: &'static str,
    pub ops: &'static [Kind],
    /// 5.5: `NEWTABLE` / `SETLIST` use the ivABC layout and `SELF` always
    /// has a constant key.
    pub v55: bool,
    /// the layouts of the numeric and the generic `for`
    pub num: ForLayout,
    pub generic: ForLayout,
}

#[derive(Clone, Copy)]
struct I {
    w: u32,
}

impl I {
    fn op(self) -> u8 {
        (self.w & 0x7F) as u8
    }
    fn a(self) -> u32 {
        (self.w >> 7) & 0xFF
    }
    fn k(self) -> bool {
        (self.w >> 15) & 1 != 0
    }
    fn b(self) -> u32 {
        (self.w >> 16) & 0xFF
    }
    fn c(self) -> u32 {
        self.w >> 24
    }
    fn bx(self) -> u32 {
        self.w >> 15
    }
    fn sbx(self) -> i32 {
        self.bx() as i32 - 65535
    }
    fn ax(self) -> u32 {
        self.w >> 7
    }
    fn sj(self) -> i64 {
        (self.w >> 7) as i64 - 16_777_215
    }
    /// 5.5 ivABC: 6-bit vB, 10-bit vC.
    fn vb(self) -> u32 {
        (self.w >> 16) & 0x3F
    }
    fn vc(self) -> u32 {
        self.w >> 22
    }
}

fn kind(d: &Dialect, w: u32) -> Option<Kind> {
    d.ops.get(I { w }.op() as usize).copied()
}

/// The luna operator for a metamethod event (`TMS` in ltm.h).
fn event_op(tm: u32) -> Option<Op> {
    Some(match tm {
        6 => Op::Add,
        7 => Op::Sub,
        8 => Op::Mul,
        9 => Op::Mod,
        10 => Op::Pow,
        11 => Op::Div,
        12 => Op::IDiv,
        13 => Op::BAnd,
        14 => Op::BOr,
        15 => Op::BXor,
        16 => Op::Shl,
        17 => Op::Shr,
        _ => return None,
    })
}

pub(super) fn translate(d: &Dialect, raw: &mut RawProto) -> Result<Lowered, String> {
    let mut lw = Lowering::new(d.name, raw.code.len(), raw.max_stack, &raw.consts);
    let mut closed = vec![false; raw.protos.len()];
    let code = &raw.code;
    let mut pc = 0;
    while pc < code.len() {
        lw.begin(pc, raw.lines.get(pc).copied().unwrap_or(0));
        let i = I { w: code[pc] };
        let Some(k) = kind(d, code[pc]) else {
            return Err(lw.err(format_args!("unknown opcode {}", i.op())));
        };
        let next = pc as i64 + 1;
        // The instruction after this one, when it is of kind `want`.
        let after = code.get(pc + 1).map(|&w| I { w });
        let follower = |want: Kind| after.filter(|x| kind(d, x.w) == Some(want));
        match k {
            Kind::Move => {
                let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
                lw.emit(enc_abc(Op::Move, a, b, 0, false)?);
            }
            Kind::LoadI | Kind::LoadF => {
                let op = if k == Kind::LoadI {
                    Op::LoadI
                } else {
                    Op::LoadF
                };
                let a = lw.r(i.a())?;
                lw.emit(enc_asbx(op, a, i.sbx())?);
            }
            Kind::LoadK => {
                let a = lw.r(i.a())?;
                lw.load_k(a, i.bx())?;
            }
            Kind::LoadKx => {
                let Some(extra) = follower(Kind::ExtraArg) else {
                    return Err(lw.err("LOADKX without its EXTRAARG"));
                };
                let a = lw.r(i.a())?;
                lw.load_k(a, extra.ax())?;
                pc += 1;
            }
            Kind::LoadFalse | Kind::LFalseSkip | Kind::LoadTrue => {
                let op = match k {
                    Kind::LoadFalse => Op::LoadFalse,
                    Kind::LFalseSkip => Op::LFalseSkip,
                    _ => Op::LoadTrue,
                };
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(op, a, 0, 0, false)?);
            }
            Kind::LoadNil => {
                let a = lw.run(i.a(), i.b() + 1)?;
                lw.emit(enc_abc(Op::LoadNil, a, i.b(), 0, false)?);
            }
            Kind::GetUpval | Kind::SetUpval => {
                let op = if k == Kind::GetUpval {
                    Op::GetUpval
                } else {
                    Op::SetUpval
                };
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(op, a, i.b(), 0, false)?);
            }
            Kind::GetTabUp => {
                let a = lw.r(i.a())?;
                lw.get_tabup(a, i.b(), i.c())?;
            }
            Kind::GetTable => {
                let (a, b, c) = (lw.r(i.a())?, lw.r(i.b())?, lw.r(i.c())?);
                lw.emit(enc_abc(Op::GetTable, a, b, c, false)?);
            }
            Kind::GetI => {
                let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
                lw.emit(enc_abc(Op::GetI, a, b, i.c(), false)?);
            }
            Kind::GetField => {
                let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
                lw.get_field(a, b, i.c())?;
            }
            Kind::SetTabUp | Kind::SetTable | Kind::SetI | Kind::SetField => store(&mut lw, k, i)?,
            Kind::NewTable => {
                let Some(extra) = follower(Kind::ExtraArg) else {
                    return Err(lw.err("NEWTABLE without its EXTRAARG"));
                };
                let a = lw.r(i.a())?;
                let (b, c, bits) = if d.v55 {
                    (i.vb(), i.vc(), 10)
                } else {
                    (i.b(), i.c(), 8)
                };
                // luna's `C` holds sizes up to 255 (see `new_table_operands`)
                let asize = u64::from(c)
                    + if i.k() {
                        u64::from(extra.ax()) << bits
                    } else {
                        0
                    };
                lw.emit(enc_abc(Op::NewTable, a, b, asize.min(0xFF) as u32, false)?);
                pc += 1;
            }
            // R[A+1] := R[B]; R[A] := R[B][RK(C)] (5.5: always K[C])
            Kind::SelfOp => self_op(&mut lw, d, i)?,
            Kind::ArithI | Kind::ArithK => const_arith(&mut lw, k, i, follower)?,
            Kind::Arith(op) => {
                let (a, b, c) = (lw.r(i.a())?, lw.r(i.b())?, lw.r(i.c())?);
                lw.emit(enc_abc(op, a, b, c, false)?);
            }
            // Consumed by the arithmetic op before it.
            Kind::MmBin | Kind::MmBinI | Kind::MmBinK => {}
            Kind::Unary(op) => {
                let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
                lw.emit(enc_abc(op, a, b, 0, false)?);
            }
            Kind::Concat => {
                let a = lw.run(i.a(), i.b().max(1))?;
                lw.emit(enc_abc(Op::Concat, a, i.b(), 0, false)?);
            }
            Kind::Close | Kind::Tbc => {
                let op = if k == Kind::Close { Op::Close } else { Op::Tbc };
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(op, a, 0, 0, false)?);
            }
            Kind::Jmp => lw.jump(enc_sj(Op::Jmp, 0)?, Jump::Jmp, next + i.sj())?,
            Kind::Eq | Kind::Lt | Kind::Le => compare(&mut lw, k, i)?,
            Kind::EqK => {
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(Op::EqK, a, i.b(), 0, i.k())?);
            }
            // if ((R[A] <op> sB) ~= k) then pc++; C: the literal was a float
            Kind::EqI | Kind::LtI | Kind::LeI | Kind::GtI | Kind::GeI => {
                let op = match k {
                    Kind::EqI => Op::EqI,
                    Kind::LtI => Op::LtI,
                    Kind::LeI => Op::LeI,
                    Kind::GtI => Op::GtI,
                    _ => Op::GeI,
                };
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(op, a, i.b(), i.c(), i.k())?);
            }
            Kind::Test => {
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(Op::Test, a, 0, 0, i.k())?);
            }
            Kind::TestSet => {
                let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
                lw.emit(enc_abc(Op::TestSet, a, b, 0, i.k())?);
            }
            Kind::Call => {
                let (b, c) = (i.b(), i.c());
                let a = lw.run(i.a(), b.max(c.saturating_sub(1)).max(1))?;
                lw.emit(enc_abc(Op::Call, a, b, c, false)?);
            }
            // C and k only matter to PUC's own frame layout for varargs.
            Kind::TailCall => {
                let a = lw.run(i.a(), i.b().max(1))?;
                lw.emit(enc_abc(Op::TailCall, a, i.b(), 0, false)?);
            }
            Kind::Return => lw.ret(i.a(), i.b())?,
            Kind::Return0 => {
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(Op::Return0, a, 0, 0, false)?);
            }
            Kind::Return1 => {
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(Op::Return1, a, 0, 0, false)?);
            }
            // FORPREP skips past its FORLOOP (at pc + 1 + Bx) when the loop
            // does not run; FORLOOP jumps back Bx.
            Kind::ForPrep | Kind::ForLoop => for_jump(&mut lw, d, k, i, next)?,
            Kind::TForPrep => {
                let a = lw.run(i.a(), d.generic.var())?;
                let target = next + i.bx() as i64;
                lw.jump(enc_abx(d.generic.ops().0, a, 0)?, Jump::TForPrep, target)?;
            }
            Kind::TForCall => {
                let a = lw.run(i.a(), d.generic.var() + i.c().max(3))?;
                lw.emit(enc_abc(d.generic.ops().1, a, 0, i.c(), false)?);
            }
            Kind::TForLoop => {
                let a = lw.run(i.a(), d.generic.var() + 1)?;
                let target = next - i.bx() as i64;
                lw.jump(enc_abx(d.generic.ops().2, a, 0)?, Jump::Back, target)?;
            }
            // R[A][C+j] := R[A+j], 1 <= j <= B; k: EXTRAARG extends C
            Kind::SetList => {
                if set_list(&mut lw, d, i, follower)? {
                    pc += 1;
                }
            }
            Kind::Closure => closure(&mut lw, &mut raw.protos, &mut closed, i)?,
            // 5.5's B/k name the vararg table, which luna keeps in the frame.
            Kind::Vararg => {
                let a = lw.run(i.a(), i.c().saturating_sub(1).max(1))?;
                lw.emit(enc_abc(Op::Vararg, a, 0, i.c(), false)?);
            }
            // R[A] := R[B][R[C]] with R[B] the (virtual) vararg table
            Kind::GetVarg => {
                let (a, c) = (lw.r(i.a())?, lw.r(i.c())?);
                lw.emit(enc_abc(Op::VargIdx, a, 0, c, false)?);
            }
            Kind::ErrNNil => {
                let a = lw.r(i.a())?;
                if i.bx() > isa::MAX_BX {
                    return Err(lw.err("ERRNNIL name index past luna's limit"));
                }
                lw.emit(enc_abx(Op::ErrNNil, a, i.bx())?);
            }
            // luna adjusts varargs at call time. 5.5 keeps the vararg
            // parameter in register `num_params`: a table built here
            // (PF_VATAB), otherwise nil. Emitting that store, rather than
            // nothing, also keeps the parameter's local — declared after
            // this instruction — from looking live at function entry.
            Kind::VarargPrep if d.v55 => vararg_prep(&mut lw, raw)?,
            Kind::VarargPrep => {}
            Kind::ExtraArg => return Err(lw.err("EXTRAARG without an instruction to extend")),
        }
        pc += 1;
    }
    lw.finish(&raw.locvars)
}
