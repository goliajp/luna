//! Lowerings of the opcodes that take more than a line or two.

use super::{Dialect, I, Kind, event_op, for_base};
use crate::vm::dump::puc::classic::is_env;
use crate::vm::dump::puc::lower::{Jump, Lowering, RawProto, enc_abc, enc_abx, enc_asbx};
use crate::vm::isa::Op;

/// `SETTABUP` / `SETTABLE` / `SETI` / `SETFIELD`.
pub(super) fn store(lw: &mut Lowering, raw: &RawProto, k: Kind, i: I) -> Result<(), String> {
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
    Ok(())
}

/// `ADDI` / `*K` arithmetic, fused with the `MMBINI` / `MMBINK` that follows it.
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
    Ok(())
}

/// `EQI` / `LTI` / `LEI` / `GTI` / `GEI`: the immediate goes through a scratch register.
pub(super) fn compare_imm(lw: &mut Lowering, k: Kind, i: I) -> Result<(), String> {
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
    Ok(())
}

/// `TFORCALL`: results land from luna's A+4, which the loop window must line up.
pub(super) fn tfor_call(lw: &mut Lowering, d: &Dialect, i: I) -> Result<(), String> {
    let a = for_base(lw, d, i.a())?;
    // luna writes the results from its A+4, PUC 5.5 from A+3.
    let first = if d.v55 { i.a() + 3 } else { i.a() + 4 };
    if lw.run(first, i.c().max(1))? != a + 4 {
        return Err(lw.err("generic-for results outside the loop's frame"));
    }
    lw.emit(enc_abc(Op::TForCall, a, 0, i.c(), false)?);
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
    let a = for_base(lw, d, i.a())?;
    if k == Kind::ForPrep {
        let target = next + i.bx() as i64;
        lw.jump(enc_abx(Op::ForPrep, a, 0)?, Jump::ForPrep, target)?;
    } else {
        let target = next - i.bx() as i64;
        lw.jump(enc_abx(Op::ForLoop, a, 0)?, Jump::Back, target)?;
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
