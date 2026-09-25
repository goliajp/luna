//! Code translation shared by PUC 5.4 and 5.5.
//!
//! luna's ISA descends from 5.4's layout (`op:7 | A:8 | k:1 | B:8 | C:8`),
//! so most opcodes map one to one. Semantics follow `lua-5.4.9/src/lvm.c`
//! and `lua-5.5.1/src/lvm.c`; what needs lowering:
//!
//! - **Immediate and constant arithmetic** (`ADDI`, `ADDK`…`BXORK`, `SHRI`,
//!   `SHLI`): luna has register operands only. The `MMBINI` / `MMBINK` that
//!   PUC always emits next records the metamethod event, the operand as
//!   written in the source, and whether the operands were swapped, so the
//!   pair becomes one luna op on the original operands and operator:
//!   `x - 1`, compiled as `ADDI x -1; MMBINI x 1 __sub`, runs as `x - 1`,
//!   which is what a `__sub` metamethod must see. The `MMBIN*` then emits
//!   nothing (luna's arithmetic ops fall back to metamethods themselves).
//! - **Immediate comparisons** (`EQI`, `LTI`, `LEI`, `GTI`, `GEI`): the
//!   immediate is loaded into a scratch register, as a float when `C` says
//!   the source literal was one; `GTI` / `GEI` swap operands.
//! - **`RK(C)` stores and `SELF`**: a constant value goes through a scratch
//!   register (luna reads store values from registers only).
//! - **`NEWTABLE`** always carries an `EXTRAARG`, which luna's `NewTable`
//!   (it ignores size hints) does not consume, so both become one op.
//! - **`VARARGPREP`**: luna adjusts varargs at call time; in 5.5 it also
//!   sets the vararg parameter's register — a table (`PF_VATAB`, becomes
//!   `GetVarg`) or nil (`LoadNil`).
//!
//! 5.5 differs from 5.4 in its opcode numbering, in 6/10-bit `NEWTABLE` /
//! `SETLIST` operands, in `SELF` always taking a constant key, and in giving
//! both kinds of `for` loop three hidden slots instead of 5.4's (and luna's)
//! four; those loops become luna's ops inside a loop window.

use super::classic::is_env;
use super::lower::{Jump, Lowered, Lowering, RawProto, Window, enc_abc, enc_abx, enc_asbx, enc_sj};
use crate::vm::isa::{self, Op};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
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
    /// 5.5: `NEWTABLE` / `SETLIST` use the ivABC layout, `SELF` always has
    /// a constant key, and `for` loops keep three hidden slots.
    pub v55: bool,
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
    /// Signed B / C: excess-127 (`OFFSET_sC`).
    fn sb(self) -> i32 {
        self.b() as i32 - 127
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

/// 5.5 loops span from their prep to their loop op; their loop variables
/// start at `A+2` in PUC and `A+3` (numeric) / `A+4` (generic) in luna.
fn loop_windows(d: &Dialect, code: &[u32]) -> Result<Vec<Window>, String> {
    if !d.v55 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (p, &w) in code.iter().enumerate() {
        let i = I { w };
        let last = match kind(d, w) {
            Some(Kind::ForPrep) => {
                let q = p + 1 + i.bx() as usize;
                (code.get(q).and_then(|&w| kind(d, w)) == Some(Kind::ForLoop)).then_some(q)
            }
            Some(Kind::TForPrep) => {
                let t = p + 1 + i.bx() as usize;
                let call = code.get(t).and_then(|&w| kind(d, w));
                let lp = code.get(t + 1).and_then(|&w| kind(d, w));
                (call == Some(Kind::TForCall) && lp == Some(Kind::TForLoop)).then_some(t + 1)
            }
            _ => continue,
        };
        let Some(last) = last else {
            return Err(format!(
                "{} chunk: loop prep without its loop (pc {p})",
                d.name
            ));
        };
        out.push(Window {
            first: p,
            last,
            pivot: i.a() + 2,
        });
    }
    Ok(out)
}

pub(super) fn translate(d: &Dialect, raw: &mut RawProto) -> Result<Lowered, String> {
    let windows = loop_windows(d, &raw.code)?;
    let mut lw = Lowering::new(d.name, raw.code.len(), raw.max_stack, windows);
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
                lw.get_tabup(a, i.b(), i.c(), is_env(raw, i.b()))?;
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
            Kind::SetTabUp | Kind::SetTable | Kind::SetI | Kind::SetField => {
                // RK(C): the value is a constant when k is set.
                let v = if i.k() {
                    lw.k_in_temp(i.c())?
                } else {
                    lw.r(i.c())?
                };
                match k {
                    Kind::SetTabUp => lw.set_tabup(i.a(), i.b(), v, is_env(raw, i.a()))?,
                    Kind::SetField => {
                        let a = lw.r(i.a())?;
                        lw.set_field(a, i.b(), v)?;
                    }
                    Kind::SetTable => {
                        let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
                        lw.emit(enc_abc(Op::SetTable, a, b, v, false)?);
                    }
                    _ => {
                        let a = lw.r(i.a())?;
                        lw.emit(enc_abc(Op::SetI, a, i.b(), v, false)?);
                    }
                }
            }
            Kind::NewTable => {
                if follower(Kind::ExtraArg).is_none() {
                    return Err(lw.err("NEWTABLE without its EXTRAARG"));
                }
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(Op::NewTable, a, 0, 0, false)?);
                pc += 1;
            }
            // R[A+1] := R[B]; R[A] := R[B][RK(C)] (5.5: always K[C])
            Kind::SelfOp => {
                let (a, b) = (lw.run(i.a(), 2)?, lw.r(i.b())?);
                if d.v55 || i.k() {
                    lw.emit(enc_abc(Op::SelfOp, a, b, i.c(), true)?);
                } else {
                    let c = lw.r(i.c())?;
                    lw.emit(enc_abc(Op::SelfOp, a, b, c, false)?);
                }
            }
            Kind::ArithI | Kind::ArithK => {
                let mm = if k == Kind::ArithI {
                    follower(Kind::MmBinI)
                } else {
                    follower(Kind::MmBinK)
                };
                let Some(mm) = mm else {
                    return Err(lw.err("constant arithmetic without its MMBINI/MMBINK"));
                };
                let Some(op) = event_op(mm.c()) else {
                    return Err(lw.err(format_args!("MMBIN event {} is not arithmetic", mm.c())));
                };
                let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
                let t = lw.temp()?;
                if k == Kind::ArithI {
                    lw.emit(enc_asbx(Op::LoadI, t, mm.sb())?);
                } else {
                    lw.load_k(t, mm.b())?;
                }
                // k on the MMBIN: the constant was the left operand.
                let (l, r) = if mm.k() { (t, b) } else { (b, t) };
                // `x - 0` ran as `ADDI x 0`: luna's flagged `Add` (see `Op::Add`)
                if k == Kind::ArithI && op == Op::Sub && mm.sb() == 0 && !mm.k() {
                    lw.emit(enc_abc(Op::Add, a, l, r, true)?);
                } else {
                    lw.emit(enc_abc(op, a, l, r, false)?);
                }
            }
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
            Kind::Eq | Kind::Lt | Kind::Le => {
                let op = match k {
                    Kind::Eq => Op::Eq,
                    Kind::Lt => Op::Lt,
                    _ => Op::Le,
                };
                let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
                lw.emit(enc_abc(op, a, b, 0, i.k())?);
            }
            Kind::EqK => {
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(Op::EqK, a, i.b(), 0, i.k())?);
            }
            // if ((R[A] <op> sB) ~= k) then pc++; C: the literal was a float
            Kind::EqI | Kind::LtI | Kind::LeI | Kind::GtI | Kind::GeI => {
                let a = lw.r(i.a())?;
                let t = lw.temp()?;
                let load = if i.c() != 0 { Op::LoadF } else { Op::LoadI };
                lw.emit(enc_asbx(load, t, i.sb())?);
                let (op, l, r) = match k {
                    Kind::EqI => (Op::Eq, a, t),
                    Kind::LtI => (Op::Lt, a, t),
                    Kind::LeI => (Op::Le, a, t),
                    Kind::GtI => (Op::Lt, t, a),
                    _ => (Op::Le, t, a),
                };
                lw.emit(enc_abc(op, l, r, 0, i.k())?);
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
            Kind::Return0 => lw.emit(enc_abc(Op::Return0, 0, 0, 0, false)?),
            Kind::Return1 => {
                let a = lw.r(i.a())?;
                lw.emit(enc_abc(Op::Return1, a, 0, 0, false)?);
            }
            // FORPREP skips past its FORLOOP (at pc + 1 + Bx) when the loop
            // does not run; FORLOOP jumps back Bx.
            Kind::ForPrep | Kind::ForLoop => {
                let a = for_base(&lw, d, i.a())?;
                if k == Kind::ForPrep {
                    let target = next + i.bx() as i64;
                    lw.jump(enc_abx(Op::ForPrep, a, 0)?, Jump::ForPrep, target)?;
                } else {
                    let target = next - i.bx() as i64;
                    lw.jump(enc_abx(Op::ForLoop, a, 0)?, Jump::Back, target)?;
                }
            }
            Kind::TForPrep => {
                let a = for_base(&lw, d, i.a())?;
                let target = next + i.bx() as i64;
                lw.jump(enc_abx(Op::TForPrep, a, 0)?, Jump::TForPrep, target)?;
            }
            Kind::TForCall => {
                let a = for_base(&lw, d, i.a())?;
                // luna writes the results from its A+4, PUC 5.5 from A+3.
                let first = if d.v55 { i.a() + 3 } else { i.a() + 4 };
                if lw.run(first, i.c().max(1))? != a + 4 {
                    return Err(lw.err("generic-for results outside the loop's frame"));
                }
                lw.emit(enc_abc(Op::TForCall, a, 0, i.c(), false)?);
            }
            Kind::TForLoop => {
                let a = for_base(&lw, d, i.a())?;
                let target = next - i.bx() as i64;
                lw.jump(enc_abx(Op::TForLoop, a, 0)?, Jump::Back, target)?;
            }
            // R[A][C+j] := R[A+j], 1 <= j <= B; k: EXTRAARG extends C
            Kind::SetList => {
                let (n, c, c_bits) = if d.v55 {
                    (i.vb(), i.vc(), 10)
                } else {
                    (i.b(), i.c(), 8)
                };
                let mut offset = c as u64;
                if i.k() {
                    let Some(extra) = follower(Kind::ExtraArg) else {
                        return Err(lw.err("SETLIST without its EXTRAARG"));
                    };
                    offset += (extra.ax() as u64) << c_bits;
                    pc += 1;
                }
                let a = lw.run(i.a(), n + 1)?;
                lw.set_list(a, n, offset)?;
            }
            Kind::Closure => {
                let idx = i.bx() as usize;
                let Some(child) = raw.protos.get_mut(idx) else {
                    return Err(lw.err(format_args!("CLOSURE of missing function {idx}")));
                };
                if std::mem::replace(&mut closed[idx], true) {
                    return Err(lw.err(format_args!("function {idx} instantiated twice")));
                }
                for u in child.upvals.iter_mut().filter(|u| u.in_stack) {
                    let r = lw.r(u.index as u32)?;
                    // `r` is at most 255: `Lowering::reg_at` refuses more.
                    u.index = r as u8;
                }
                let a = lw.r(i.a())?;
                lw.emit(enc_abx(Op::Closure, a, idx as u32)?);
            }
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
            Kind::VarargPrep if d.v55 => {
                let a = lw.r(raw.num_params as u32)?;
                if raw.vararg_table {
                    lw.emit(enc_abc(Op::GetVarg, a, 0, 0, false)?);
                } else {
                    lw.emit(enc_abc(Op::LoadNil, a, 0, 0, false)?);
                }
            }
            Kind::VarargPrep => {}
            Kind::ExtraArg => return Err(lw.err("EXTRAARG without an instruction to extend")),
        }
        pc += 1;
    }
    lw.finish(&raw.locvars)
}

/// luna register of a `for` loop's base `A`, after checking that luna's
/// four hidden slots fit the frame. In 5.5 the loop window has already moved
/// PUC's third slot (`A+2`) up to luna's fourth.
fn for_base(lw: &Lowering, d: &Dialect, a: u32) -> Result<u32, String> {
    let base = lw.r(a)?;
    let last = if d.v55 { a + 2 } else { a + 3 };
    if lw.r(last)? != base + 3 {
        return Err(lw.err("loop slots straddle another loop's window"));
    }
    Ok(base)
}
