//! The other way around a short `if` without an `else`: when a comparison's
//! unrecorded branch only differs from the recorded one by a few register
//! moves and loads, the trace takes that branch itself and rejoins, instead
//! of leaving for the interpreter at every pass that goes the other way.

use super::*;

/// The unrecorded branch of the comparison at a recorded op.
#[derive(Clone, Debug)]
pub(super) enum AltPath {
    /// The recording took the `Jmp`; the other way runs `ops` (the code
    /// between the `Jmp` and its target) and goes on with the op after the
    /// `Jmp`.
    Run(Vec<Inst>),
    /// The recording ran the ops up to the `Jmp`'s target, the recorded op
    /// `join`; the other way goes straight there. `writes` are the registers
    /// those ops write.
    Skip { join: usize, writes: Vec<u32> },
}

/// The most ops a branch may run or skip.
const MAX_ALT_OPS: usize = 4;

/// A register move or a load of a number: what a branch may run or skip.
fn simple(inst: Inst, proto: &Proto) -> bool {
    match inst.op() {
        Op::Move | Op::LoadI | Op::LoadF => true,
        Op::LoadK => matches!(
            proto.consts.get(inst.bx() as usize),
            Some(luna_core::runtime::Value::Int(_) | luna_core::runtime::Value::Float(_))
        ),
        _ => false,
    }
}

/// The kind `inst` (a [`simple`] op) leaves in its `R[A]`, the registers
/// holding `kinds` (frame-relative).
pub(super) fn simple_kind(inst: Inst, proto: &Proto, kinds: &[RegKind]) -> RegKind {
    match inst.op() {
        Op::Move => k_op(kinds, inst.b()),
        Op::LoadI => RegKind::Int,
        Op::LoadF => RegKind::Float,
        Op::LoadK => match proto.consts[inst.bx() as usize] {
            luna_core::runtime::Value::Int(_) => RegKind::Int,
            _ => RegKind::Float,
        },
        _ => unreachable!("not a simple op"),
    }
}

/// Whether running `ops` from register kinds `kinds` leaves every register
/// it writes with the kind it has in `kinds`, so that both ways rejoin with
/// one kind per register.
pub(super) fn keeps_kinds(ops: &[Inst], proto: &Proto, kinds: &[RegKind]) -> bool {
    let mut k = kinds.to_vec();
    for &inst in ops {
        let r = simple_kind(inst, proto, &k);
        let a = inst.a() as usize;
        if r.untyped() || a >= k.len() || k[a] != r {
            return false;
        }
        k[a] = r;
    }
    true
}

/// For each comparison among the recorded ops before `end`, its other way
/// when the trace can take it (depth 0 of the head function only).
pub(super) fn find_alt_paths(
    record: &TraceRecord,
    end: usize,
    head_proto: Gc<Proto>,
) -> Vec<Option<AltPath>> {
    let ops = &record.ops;
    let code = &head_proto.code;
    let at_head =
        |r: &RecordedOp| r.inline_depth == 0 && std::ptr::eq(r.proto.as_ptr(), head_proto.as_ptr());
    (0..end)
        .map(|i| {
            let rop = &ops[i];
            if !at_head(rop) || !matches!(rop.inst.op(), Op::Lt | Op::Le | Op::Eq | Op::EqK) {
                return None;
            }
            let pc = rop.pc as usize;
            let jmp = *code.get(pc + 1)?;
            if jmp.op() != Op::Jmp || jmp.sj() <= 0 {
                return None;
            }
            let target = pc + 2 + jmp.sj() as usize;
            let n = target - (pc + 2);
            if n > MAX_ALT_OPS {
                return None;
            }
            let next = ops.get(i + 1).filter(|r| at_head(r))?;
            if next.pc as usize == pc + 1 {
                // took the Jmp: the code it jumped over is the other way
                let rejoin = ops.get(i + 2).filter(|r| at_head(r))?;
                if i + 2 > end || rejoin.pc as usize != target {
                    return None;
                }
                let alt = code[pc + 2..target].to_vec();
                alt.iter()
                    .all(|&inst| simple(inst, &head_proto))
                    .then_some(AltPath::Run(alt))
            } else if next.pc as usize == pc + 2 && rop.inst.op() != Op::EqK {
                // ran the code up to the Jmp's target: skipping it is the
                // other way
                let join = i + 1 + n;
                if join > end {
                    return None;
                }
                let ran = ops.get(i + 1..=join)?;
                let straight = ran[..n].iter().enumerate().all(|(m, r)| {
                    at_head(r) && r.pc as usize == pc + 2 + m && simple(r.inst, &head_proto)
                });
                if !straight || !at_head(&ran[n]) || ran[n].pc as usize != target {
                    return None;
                }
                let writes = ran[..n].iter().map(|r| r.inst.a()).collect();
                Some(AltPath::Skip { join, writes })
            } else {
                None
            }
        })
        .collect()
}

/// The registers the other ways write, which the trace must hold from the
/// entry on so that both ways rejoin with a value in each.
pub(super) fn alt_written(alts: &[Option<AltPath>]) -> Vec<u32> {
    alts.iter()
        .flatten()
        .flat_map(|a| match a {
            AltPath::Run(ops) => ops.iter().map(|i| i.a()).collect::<Vec<_>>(),
            AltPath::Skip { writes, .. } => writes.clone(),
        })
        .collect()
}
