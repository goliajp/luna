//! Code translation shared by PUC 5.2 and 5.3.
//!
//! The two instruction sets share one layout (`op:6 | A:8 | C:9 | B:9`,
//! RK operands, `EXTRAARG`) and one set of semantics (`lua-5.2.4/src/lvm.c`,
//! `lua-5.3.6/src/lvm.c`); 5.3 renumbers the opcodes and adds integer
//! division and the bitwise operators. Each dialect module maps its opcode
//! numbers onto [`Kind`] and calls [`translate`].
//!
//! What differs from luna, beyond the RK operands [`super::lower`] handles:
//!
//! - `JMP A sBx` with `A > 0` also closes upvalues from `R(A-1)`.
//! - `LOADNIL A B` clears `A..=A+B`, the same run as luna's.
//! - The generic `for` keeps three hidden slots: `TFORCALL A C` writes the
//!   loop variables at `A+3`, and `TFORLOOP` names the control slot `A+2`.
//!   Both become luna's ops inside a loop window over the body.
//! - `SETLIST` counts 50-field blocks from 1; `C = 0` takes the block number
//!   from the `EXTRAARG` that follows.

use super::lower::{Jump, Lowered, Lowering, RawProto, Window};
use crate::vm::isa::{Inst, Op};

/// An opcode's meaning, independent of the dialect's numbering.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Move,
    LoadK,
    LoadKx,
    LoadBool,
    LoadNil,
    GetUpval,
    GetTabUp,
    GetTable,
    SetTabUp,
    SetUpval,
    SetTable,
    NewTable,
    SelfOp,
    /// `R(A) := RK(B) op RK(C)`
    Arith(Op),
    /// `R(A) := op R(B)`
    Unary(Op),
    Concat,
    Jmp,
    Eq,
    Lt,
    Le,
    Test,
    TestSet,
    Call,
    TailCall,
    Return,
    ForLoop,
    ForPrep,
    TForCall,
    TForLoop,
    SetList,
    Closure,
    Vararg,
    ExtraArg,
}

#[derive(Clone, Copy)]
struct I {
    op: u8,
    a: u32,
    b: u32,
    c: u32,
}

impl I {
    fn decode(w: u32) -> I {
        I {
            op: (w & 0x3F) as u8,
            a: (w >> 6) & 0xFF,
            c: (w >> 14) & 0x1FF,
            b: (w >> 23) & 0x1FF,
        }
    }
    fn bx(self) -> u32 {
        (self.b << 9) | self.c
    }
    fn sbx(self) -> i64 {
        self.bx() as i64 - 131071
    }
}

fn ax(w: u32) -> u32 {
    w >> 6
}

/// `LFIELDS_PER_FLUSH` (lopcodes.h, both versions).
const FIELDS_PER_FLUSH: u64 = 50;

fn kind(ops: &[Kind], w: u32) -> Option<Kind> {
    ops.get(I::decode(w).op as usize).copied()
}

/// `TFORLOOP A sBx` at `p` closes a body running from its jump target to
/// `p`; the loop variables start at `A+1` (`TFORCALL`'s `A+3`).
fn loop_windows(dialect: &str, code: &[u32], ops: &[Kind]) -> Result<Vec<Window>, String> {
    let mut out = Vec::new();
    for (p, &w) in code.iter().enumerate() {
        if kind(ops, w) != Some(Kind::TForLoop) {
            continue;
        }
        let i = I::decode(w);
        let body = p as i64 + 1 + i.sbx();
        if !(0..=p as i64).contains(&body) {
            return Err(format!(
                "{dialect} chunk: TFORLOOP jumps to {body} (pc {p})"
            ));
        }
        out.push(Window {
            first: body as usize,
            last: p,
            pivot: i.a + 1,
        });
    }
    Ok(out)
}

/// Whether upvalue `up` is the environment (by name, as PUC's `isEnv`).
pub(super) fn is_env(raw: &RawProto, up: u32) -> bool {
    raw.upvals
        .get(up as usize)
        .is_some_and(|u| &*u.name == "_ENV")
}

pub(super) fn translate(
    dialect: &'static str,
    ops: &[Kind],
    raw: &mut RawProto,
) -> Result<Lowered, String> {
    let windows = loop_windows(dialect, &raw.code, ops)?;
    let mut lw = Lowering::new(dialect, raw.code.len(), raw.max_stack, windows);
    let mut closed = vec![false; raw.protos.len()];
    let code = &raw.code;
    let mut pc = 0;
    while pc < code.len() {
        lw.begin(pc, raw.lines.get(pc).copied().unwrap_or(0));
        let i = I::decode(code[pc]);
        let Some(k) = kind(ops, code[pc]) else {
            return Err(lw.err(format_args!("unknown opcode {}", i.op)));
        };
        let next = pc as i64 + 1;
        match k {
            Kind::Move => {
                let (a, b) = (lw.r(i.a)?, lw.r(i.b)?);
                lw.emit(Inst::iabc(Op::Move, a, b, 0, false));
            }
            Kind::LoadK => {
                let a = lw.r(i.a)?;
                lw.load_k(a, i.bx())?;
            }
            Kind::LoadKx => {
                pc += 1;
                let a = lw.r(i.a)?;
                match code.get(pc) {
                    Some(&w) if kind(ops, w) == Some(Kind::ExtraArg) => lw.load_k(a, ax(w))?,
                    _ => return Err(lw.err("LOADKX without its EXTRAARG")),
                }
            }
            Kind::LoadBool => {
                let a = lw.r(i.a)?;
                match (i.b != 0, i.c != 0) {
                    (false, false) => lw.emit(Inst::iabc(Op::LoadFalse, a, 0, 0, false)),
                    (false, true) => lw.emit(Inst::iabc(Op::LFalseSkip, a, 0, 0, false)),
                    (true, false) => lw.emit(Inst::iabc(Op::LoadTrue, a, 0, 0, false)),
                    (true, true) => {
                        lw.emit(Inst::iabc(Op::LoadTrue, a, 0, 0, false));
                        lw.jump(Inst::isj(Op::Jmp, 0), Jump::Jmp, next + 1)?;
                    }
                }
            }
            Kind::LoadNil => {
                let b = lw.byte(i.b, "LOADNIL B")?;
                let a = lw.run(i.a, b + 1)?;
                lw.emit(Inst::iabc(Op::LoadNil, a, b, 0, false));
            }
            Kind::GetUpval => {
                let (a, b) = (lw.r(i.a)?, lw.byte(i.b, "GETUPVAL B")?);
                lw.emit(Inst::iabc(Op::GetUpval, a, b, 0, false));
            }
            Kind::SetUpval => {
                let (a, b) = (lw.r(i.a)?, lw.byte(i.b, "SETUPVAL B")?);
                lw.emit(Inst::iabc(Op::SetUpval, a, b, 0, false));
            }
            // R(A) := UpValue[B][RK(C)]
            Kind::GetTabUp => {
                let (a, up) = (lw.r(i.a)?, lw.byte(i.b, "GETTABUP B")?);
                if i.c & super::lower::RK_BIT != 0 {
                    lw.get_tabup(a, up, i.c & 0xFF, is_env(raw, up))?;
                } else {
                    let (t, key) = (lw.temp()?, lw.r(i.c)?);
                    lw.emit(Inst::iabc(Op::GetUpval, t, up, 0, false));
                    lw.emit(Inst::iabc(Op::GetTable, a, t, key, false));
                }
            }
            // UpValue[A][RK(B)] := RK(C)
            Kind::SetTabUp => {
                let v = lw.rk(i.c)?;
                if i.b & super::lower::RK_BIT != 0 {
                    lw.set_tabup(i.a, i.b & 0xFF, v, is_env(raw, i.a))?;
                } else {
                    let (t, key) = (lw.temp()?, lw.r(i.b)?);
                    lw.emit(Inst::iabc(Op::GetUpval, t, i.a, 0, false));
                    lw.emit(Inst::iabc(Op::SetTable, t, key, v, false));
                }
            }
            Kind::GetTable => {
                let (a, b) = (lw.r(i.a)?, lw.r(i.b)?);
                lw.get_table_rk(a, b, i.c)?;
            }
            Kind::SetTable => {
                let a = lw.r(i.a)?;
                lw.set_table_rk(a, i.b, i.c)?;
            }
            // luna's NewTable ignores its size hints.
            Kind::NewTable => {
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabc(Op::NewTable, a, 0, 0, false));
            }
            Kind::SelfOp => lw.self_rk(i.a, i.b, i.c)?,
            Kind::Arith(op) => {
                let a = lw.r(i.a)?;
                let b = lw.rk(i.b)?;
                let c = lw.rk(i.c)?;
                lw.emit(Inst::iabc(op, a, b, c, false));
            }
            Kind::Unary(op) => {
                let (a, b) = (lw.r(i.a)?, lw.r(i.b)?);
                lw.emit(Inst::iabc(op, a, b, 0, false));
            }
            Kind::Concat => lw.concat_range(i.a, i.b, i.c)?,
            // pc += sBx; if (A) close all upvalues >= R(A - 1)
            Kind::Jmp => {
                let target = next + i.sbx();
                if i.a == 0 {
                    lw.jump(Inst::isj(Op::Jmp, 0), Jump::Jmp, target)?;
                } else {
                    let close = lw.r(i.a - 1)?;
                    let guarded = pc > 0
                        && matches!(
                            kind(ops, code[pc - 1]),
                            Some(Kind::Eq | Kind::Lt | Kind::Le | Kind::Test | Kind::TestSet)
                        );
                    if guarded {
                        lw.jump_closing(close, target)?;
                    } else {
                        lw.emit(Inst::iabc(Op::Close, close, 0, 0, false));
                        lw.jump(Inst::isj(Op::Jmp, 0), Jump::Jmp, target)?;
                    }
                }
            }
            Kind::Eq => lw.compare_rk(Op::Eq, i.a != 0, i.b, i.c)?,
            Kind::Lt => lw.compare_rk(Op::Lt, i.a != 0, i.b, i.c)?,
            Kind::Le => lw.compare_rk(Op::Le, i.a != 0, i.b, i.c)?,
            // TEST A C: if not (R(A) <=> C) then pc++
            Kind::Test => {
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabc(Op::Test, a, 0, 0, i.c != 0));
            }
            // TESTSET A B C: if (R(B) <=> C) then R(A) := R(B) else pc++
            Kind::TestSet => {
                let (a, b) = (lw.r(i.a)?, lw.r(i.b)?);
                lw.emit(Inst::iabc(Op::TestSet, a, b, 0, i.c != 0));
            }
            Kind::Call => {
                let (b, c) = (lw.byte(i.b, "CALL B")?, lw.byte(i.c, "CALL C")?);
                let a = lw.run(i.a, b.max(c.saturating_sub(1)).max(1))?;
                lw.emit(Inst::iabc(Op::Call, a, b, c, false));
            }
            Kind::TailCall => {
                let b = lw.byte(i.b, "TAILCALL B")?;
                let a = lw.run(i.a, b.max(1))?;
                lw.emit(Inst::iabc(Op::TailCall, a, b, 0, false));
            }
            Kind::Return => {
                let b = lw.byte(i.b, "RETURN B")?;
                let a = lw.run(i.a, b.saturating_sub(1).max(1))?;
                lw.emit(Inst::iabc(Op::Return, a, b, 0, false));
            }
            // FORPREP jumps to its FORLOOP; FORLOOP jumps back to the body.
            Kind::ForPrep => {
                let a = lw.run(i.a, 4)?;
                lw.jump(Inst::iabx(Op::ForPrep, a, 0), Jump::ForPrep, next + i.sbx())?;
            }
            Kind::ForLoop => {
                let a = lw.run(i.a, 4)?;
                lw.jump(Inst::iabx(Op::ForLoop, a, 0), Jump::Back, next + i.sbx())?;
            }
            Kind::TForCall => {
                let c = lw.byte(i.c, "TFORCALL C")?;
                let a = lw.run(i.a, 3)?;
                lw.run(i.a + 3, c.max(1))?;
                lw.emit(Inst::iabc(Op::TForCall, a, 0, c, false));
            }
            // if R(A+1) ~= nil then { R(A) := R(A+1); pc += sBx }, where A is
            // the TFORCALL's A + 2.
            Kind::TForLoop => {
                let Some(base) = i.a.checked_sub(2) else {
                    return Err(lw.err(format_args!("TFORLOOP A={} below 2", i.a)));
                };
                let a = lw.run(base, 3)?;
                lw.jump(Inst::iabx(Op::TForLoop, a, 0), Jump::Back, next + i.sbx())?;
            }
            Kind::SetList => {
                let block = if i.c == 0 {
                    pc += 1;
                    match code.get(pc) {
                        Some(&w) if kind(ops, w) == Some(Kind::ExtraArg) => ax(w),
                        _ => return Err(lw.err("SETLIST without its EXTRAARG")),
                    }
                } else {
                    i.c
                };
                if block == 0 {
                    return Err(lw.err("SETLIST block number 0"));
                }
                let a = lw.run(i.a, i.b + 1)?;
                lw.set_list(a, i.b, (block as u64 - 1) * FIELDS_PER_FLUSH)?;
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
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabx(Op::Closure, a, idx as u32));
            }
            Kind::Vararg => {
                let b = lw.byte(i.b, "VARARG B")?;
                let a = lw.run(i.a, b.saturating_sub(1).max(1))?;
                lw.emit(Inst::iabc(Op::Vararg, a, 0, b, false));
            }
            Kind::ExtraArg => return Err(lw.err("EXTRAARG without an instruction to extend")),
        }
        pc += 1;
    }
    lw.finish(&raw.locvars)
}
