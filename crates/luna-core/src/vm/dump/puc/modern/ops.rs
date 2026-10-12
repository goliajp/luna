//! Lowerings of the opcodes that take more than a line or two.

use super::{Dialect, I, Kind, event_op};
use crate::vm::dump::puc::lower::{Jump, Lowering, RawProto, enc_abc, enc_abx};
use crate::vm::isa::Op;

/// `SETTABUP` / `SETTABLE` / `SETI` / `SETFIELD`; `k`: the value is
/// `K[C]`.
pub(super) fn store(lw: &mut Lowering, k: Kind, i: I) -> Result<(), String> {
    let v = if i.k() {
        (i.c(), true)
    } else {
        (lw.r(i.c())?, false)
    };
    match k {
        Kind::SetTabUp => lw.set_tabup(i.a(), i.b(), v)?,
        Kind::SetField => {
            let a = lw.r(i.a())?;
            lw.set_field(a, i.b(), v)?;
        }
        Kind::SetTable => {
            let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
            lw.emit(enc_abc(Op::SetTable, a, b, v.0, v.1)?);
        }
        _ => {
            let a = lw.r(i.a())?;
            lw.emit(enc_abc(Op::SetI, a, i.b(), v.0, v.1)?);
        }
    }
    Ok(())
}

/// `ADDI` / `SHRI` / `SHLI` / `*K` arithmetic, fused with the `MMBINI` /
/// `MMBINK` that follows it: the operator and the operand as the source
/// wrote them, and on which side (`k` of the `MMBIN*`).
pub(super) fn const_arith(
    lw: &mut Lowering,
    k: Kind,
    i: I,
    follower: impl Fn(Kind) -> Option<I>,
) -> Result<(), String> {
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
    if k == Kind::ArithK {
        let kop = op.k_form().expect("an arithmetic op");
        lw.emit(enc_abc(kop, a, b, mm.b(), mm.k())?);
        return Ok(());
    }
    // the immediate as written: `x - 1` is `ADDI x -1` with `MMBINI 1 __sub`
    let iop = match op {
        Op::Add => Op::AddI,
        Op::Sub => Op::SubI,
        Op::Shr => Op::ShrI,
        Op::Shl => Op::ShlI,
        _ => return Err(lw.err("an immediate operand of a non-additive operator")),
    };
    lw.emit(enc_abc(iop, a, b, mm.b(), mm.k())?);
    Ok(())
}

/// `CLOSURE`: remaps the child's in-stack upvalue registers to luna's frame.
pub(super) fn closure(
    lw: &mut Lowering,
    protos: &mut [RawProto],
    closed: &mut [bool],
    i: I,
) -> Result<(), String> {
    let idx = i.bx() as usize;
    let Some(child) = protos.get_mut(idx) else {
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
    Ok(())
}

/// `SELF`: R[A+1] := R[B]; R[A] := R[B][RK(C)] (5.5: always K[C]).
pub(super) fn self_op(lw: &mut Lowering, d: &Dialect, i: I) -> Result<(), String> {
    let (a, b) = (lw.run(i.a(), 2)?, lw.r(i.b())?);
    if d.v55 || i.k() {
        lw.self_k(a, b, i.c())?;
    } else {
        let c = lw.r(i.c())?;
        lw.emit(enc_abc(Op::SelfOp, a, b, c, false)?);
    }
    Ok(())
}

/// `EQ` / `LT` / `LE` on two registers.
pub(super) fn compare(lw: &mut Lowering, k: Kind, i: I) -> Result<(), String> {
    let op = match k {
        Kind::Eq => Op::Eq,
        Kind::Lt => Op::Lt,
        _ => Op::Le,
    };
    let (a, b) = (lw.r(i.a())?, lw.r(i.b())?);
    lw.emit(enc_abc(op, a, b, 0, i.k())?);
    Ok(())
}

/// `FORPREP` / `FORLOOP` with their jumps left for `Lowering::finish` to resolve.
pub(super) fn for_jump(
    lw: &mut Lowering,
    d: &Dialect,
    k: Kind,
    i: I,
    next: i64,
) -> Result<(), String> {
    let a = lw.run(i.a(), d.num.var() + 1)?;
    let (prep, _, back) = d.num.ops();
    if k == Kind::ForPrep {
        let target = next + i.bx() as i64;
        lw.jump(enc_abx(prep, a, 0)?, Jump::ForPrep, target)?;
    } else {
        let target = next - i.bx() as i64;
        lw.jump(enc_abx(back, a, 0)?, Jump::Back, target)?;
    }
    Ok(())
}

/// 5.5 `VARARGPREP`: stores the vararg parameter (a table under PF_VATAB, else nil).
pub(super) fn vararg_prep(lw: &mut Lowering, raw: &RawProto) -> Result<(), String> {
    let a = lw.r(raw.num_params as u32)?;
    if raw.vararg_table {
        lw.emit(enc_abc(Op::GetVarg, a, 0, 0, false)?);
    } else {
        lw.emit(enc_abc(Op::LoadNil, a, 0, 0, false)?);
    }
    Ok(())
}

/// `SETLIST`; returns whether it consumed the `EXTRAARG` after it.
pub(super) fn set_list(
    lw: &mut Lowering,
    d: &Dialect,
    i: I,
    follower: impl Fn(Kind) -> Option<I>,
) -> Result<bool, String> {
    let (n, c, c_bits) = if d.v55 {
        (i.vb(), i.vc(), 10)
    } else {
        (i.b(), i.c(), 8)
    };
    let mut offset = c as u64;
    let mut consumed = false;
    if i.k() {
        let Some(extra) = follower(Kind::ExtraArg) else {
            return Err(lw.err("SETLIST without its EXTRAARG"));
        };
        offset += (extra.ax() as u64) << c_bits;
        consumed = true;
    }
    let a = lw.run(i.a(), n + 1)?;
    lw.set_list(a, n, offset)?;
    Ok(consumed)
}
