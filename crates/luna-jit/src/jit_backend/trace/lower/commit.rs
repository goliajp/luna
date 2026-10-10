//! Where a trace's body commits its register writes to reg_state: before
//! an op that may read reg_state (one that is not [`pure_op`]) everything,
//! before any other op the registers no op writes again before then. Exits
//! store what they leave with, so a value is stored as it is made unless a
//! later one replaces it before anything reads reg_state.

use super::*;

/// [`sync_reg_state`] for the registers `skip` does not mark.
pub(super) fn sync_reg_state_except<E: Emit>(
    bcx: &mut E,
    regs: &[Variable],
    stored: &mut [Option<Value>],
    reg_state: Value,
    skip: &[bool],
) {
    for (idx, v) in regs.iter().copied().enumerate() {
        if skip.get(idx).copied().unwrap_or(false) {
            continue;
        }
        let val = use_var_resolved(bcx, v);
        if stored[idx] == Some(val) {
            continue;
        }
        bcx.ins()
            .store(MemFlagsData::new(), val, reg_state, (idx as i32) * 8);
        stored[idx] = Some(val);
    }
}

/// Whether everything is committed to reg_state before op `i` (see
/// `emit_body`).
pub(super) fn syncs_before(record: &TraceRecord, pl: &Plan<'_>, i: usize) -> bool {
    !pure_op(record.ops[i].inst.op()) || pl.alt_paths.get(i).is_some_and(|a| a.is_some())
}

/// Per register: whether op `i` writes it (none past the recorded ops).
pub(super) fn writes_of(n: usize, record: &TraceRecord, pl: &Plan<'_>, i: usize) -> Vec<bool> {
    let mut w = vec![false; n];
    if let (Some(rop), Some(&off)) = (record.ops.get(i), pl.op_offsets.get(i)) {
        for r in op_writes_at_offset(rop, off) {
            if let Some(x) = w.get_mut(r as usize) {
                *x = true;
            }
        }
    }
    w
}

/// Per op below `end`: the registers it or a later op writes before the
/// next op everything is committed before ([`syncs_before`]).
pub(super) fn rewritten_before_sync(
    n: usize,
    record: &TraceRecord,
    pl: &Plan<'_>,
    end: usize,
) -> Vec<Vec<bool>> {
    let mut rewritten: Vec<Vec<bool>> = vec![Vec::new(); end];
    for i in (0..end).rev() {
        let mut w = writes_of(n, record, pl, i);
        if i + 1 < end && !syncs_before(record, pl, i + 1) {
            for (x, &y) in w.iter_mut().zip(&rewritten[i + 1]) {
                *x |= y;
            }
        }
        rewritten[i] = w;
    }
    rewritten
}

/// Ops whose lowering neither calls a helper that reads reg_state or the
/// Lua stack nor lets the collector run: arithmetic, comparisons, moves,
/// loads of constants and the numeric `for` step. Their guards leave
/// through exits, which store what they leave with.
pub(super) fn pure_op(op: Op) -> bool {
    matches!(
        op,
        Op::Move
            | Op::LoadI
            | Op::LoadF
            | Op::LoadK
            | Op::LoadNil
            | Op::LoadFalse
            | Op::LoadTrue
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::IDiv
            | Op::Mod
            | Op::AddI
            | Op::AddK
            | Op::SubK
            | Op::MulK
            | Op::DivK
            | Op::IDivK
            | Op::ModK
            | Op::Unm
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr
            | Op::BAndK
            | Op::BOrK
            | Op::BXorK
            | Op::ShrI
            | Op::ShlI
            | Op::BNot
            | Op::Not
            | Op::Lt
            | Op::Le
            | Op::Eq
            | Op::LtI
            | Op::LeI
            | Op::GtI
            | Op::GeI
            | Op::EqI
            | Op::EqK
            | Op::Test
            | Op::Jmp
            | Op::ForLoop
    )
}
