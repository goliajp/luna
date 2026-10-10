//! Where a trace's body commits its register writes to reg_state. Before
//! an op that may read reg_state (one that is not [`pure_op`]) everything.
//! Exits store what they leave with. Between those:
//! - for the baseline tier, a value is stored as it is made, unless an op
//!   writes the register again before anything reads reg_state; holding
//!   it longer would make the baseline allocator spill;
//! - for the optimizing tiers (`Lower::at_exits`), nothing: values stay in
//!   registers across the back edge, which keeps only the promise that
//!   registers the body never writes hold at the head what reg_state holds.

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
    mark_writes(&mut w, record, pl, i);
    w
}

/// Sets `w[r]` for each register `r` op `i` writes.
fn mark_writes(w: &mut [bool], record: &TraceRecord, pl: &Plan<'_>, i: usize) {
    if let (Some(rop), Some(&off)) = (record.ops.get(i), pl.op_offsets.get(i)) {
        for r in op_writes_at_offset(rop, off) {
            if let Some(x) = w.get_mut(r as usize) {
                *x = true;
            }
        }
    }
}

/// Per op below `end`, a row of `n` flags: the registers it or a later op
/// writes before the next op everything is committed before
/// ([`syncs_before`]). Row `i` is `[i * n..(i + 1) * n]`.
pub(super) fn rewritten_before_sync(
    n: usize,
    record: &TraceRecord,
    pl: &Plan<'_>,
    end: usize,
) -> Vec<bool> {
    let mut rewritten = vec![false; end * n];
    for i in (0..end).rev() {
        let (row, later) = rewritten[i * n..].split_at_mut(n);
        mark_writes(row, record, pl, i);
        if i + 1 < end && !syncs_before(record, pl, i + 1) {
            for (x, &y) in row.iter_mut().zip(&later[..n]) {
                *x |= y;
            }
        }
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

/// Sets what reg_state holds at the loop head (`Lower::stored`), before
/// the body.
pub(super) fn commit_start<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    lw.stored.reserve(lw.regs_full.len());
    if !lw.at_exits {
        for &v in &lw.regs_full {
            let val = use_var_resolved(&mut lw.bcx, v);
            lw.stored.push(Some(val));
        }
        return;
    }
    let mut written = vec![false; lw.regs_full.len()];
    for w in compute_body_writes(pl.record, &pl.op_offsets) {
        if let Some(x) = written.get_mut(w as usize) {
            *x = true;
        }
    }
    for (idx, x) in written.iter_mut().enumerate() {
        // inline frames' windows: written by the calls the trace inlines
        *x |= idx >= pl.max_stack;
    }
    for (idx, &v) in lw.regs_full.iter().enumerate() {
        let val = use_var_resolved(&mut lw.bcx, v);
        let promise = (!written[idx]).then_some(val);
        lw.stored.push(promise);
        lw.head_stored.push(promise);
    }
}

/// The commit before body op `i`; `rewritten`: the registers it or a later
/// op writes before the next full commit ([`rewritten_before_sync`]).
pub(super) fn commit_before<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    i: usize,
    rewritten: &[bool],
) {
    let reg_state = lw.reg_state;
    if syncs_before(pl.record, pl, i) {
        sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
    } else if !lw.at_exits {
        sync_reg_state_except(
            &mut lw.bcx,
            &lw.regs_full,
            &mut lw.stored,
            reg_state,
            rewritten,
        );
    }
}

/// The commit after the last body op, `end`, before the tail: everything
/// for the tails that call helpers reading reg_state; otherwise, for the
/// baseline tier, what the closing op does not write again.
pub(super) fn commit_end<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, end: usize) {
    let record = pl.record;
    let reg_state = lw.reg_state;
    let generic_for = pl
        .for_loop_idx_opt
        .is_some_and(|k| record.ops[k].inst.op() == Op::TForLoop);
    if generic_for || pl.downrec_idx_opt.is_some() || pl.self_link_idx_opt.is_some() {
        sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
    } else if !lw.at_exits {
        let closing = writes_of(lw.regs_full.len(), record, pl, end);
        sync_reg_state_except(
            &mut lw.bcx,
            &lw.regs_full,
            &mut lw.stored,
            reg_state,
            &closing,
        );
    }
}

/// The commit at the back edge: everything for the baseline tier; for the
/// optimizing tiers, the head's promise (`Lower::head_stored`), storing a
/// register the body was not expected to write but did.
pub(super) fn commit_back_edge<E: Emit>(lw: &mut Lower<E>) {
    let reg_state = lw.reg_state;
    if !lw.at_exits {
        sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
        return;
    }
    for idx in 0..lw.regs_full.len() {
        if let Some(hv) = lw.head_stored[idx] {
            let v = use_var_resolved(&mut lw.bcx, lw.regs_full[idx]);
            if v != hv && lw.stored[idx] != Some(v) {
                lw.bcx
                    .ins()
                    .store(MemFlagsData::new(), v, reg_state, (idx as i32) * 8);
                lw.stored[idx] = Some(v);
            }
        }
    }
}
