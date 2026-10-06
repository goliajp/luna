//! Where a trace tests that a table it stores into is not read-only
//! (`Table::is_readonly`) before an inline store.
//!
//! A table read-only on entry is tested once, before the loop head, when
//! the trace stores into it through a head-frame register that nothing in
//! the trace writes and the table is there on entry: every iteration then
//! stores into that same table. Any other table is tested at the first
//! store into each SSA value, and not again until something could have
//! changed it. Only host code can mark a table (`Vm::set_readonly`), and
//! inside a trace host code runs in two helpers only: the concatenation
//! (a `__concat` metamethod, or a `__gc` finalizer of the collection its
//! allocation may run) and the generic-for call (a native iterator, or a
//! finalizer). The other helpers neither call metamethods nor collect;
//! calls end a trace, and the side traces an exit calls are entered
//! through their own entry. So after those two ops every table is tested
//! again, and a trace that has either one tests nothing before the loop
//! head. The stores that a test does not cover go through the store
//! helpers, which refuse a read-only table themselves.

use super::*;

/// Whether `op` may run host code (see the module comment).
fn runs_host_code(op: Op) -> bool {
    matches!(op, Op::Concat | Op::TForCall)
}

/// The registers some op of the trace writes, absolute; an op that writes
/// up to the stack top (a call or a vararg with a variable count) writes
/// everything from its first register on.
fn written_registers(pl: &Plan<'_>) -> Vec<bool> {
    let Plan {
        record,
        window_size_us,
        ..
    } = *pl;
    let mut w = vec![false; window_size_us];
    let mut mark = |from: usize, to: usize| {
        for slot in w.iter_mut().take(to).skip(from) {
            *slot = true;
        }
    };
    for (i, rop) in record.ops.iter().enumerate() {
        let off = pl.op_offsets.get(i).copied().unwrap_or(0) as usize;
        let a = off + rop.inst.a() as usize;
        let open = match rop.inst.op() {
            Op::Call => rop.inst.c() == 0,
            Op::Vararg | Op::GetVarg => true,
            _ => false,
        };
        if open {
            mark(a, window_size_us);
        }
        for r in op_writes_at_offset(rop, off as u32) {
            mark(r as usize, r as usize + 1);
        }
    }
    for alt in pl.alt_paths.iter().flatten() {
        match alt {
            alt_path::AltPath::Run(ops) => {
                for inst in ops {
                    mark(inst.a() as usize, inst.a() as usize + 1);
                }
            }
            alt_path::AltPath::Skip { writes, .. } => {
                for &r in writes {
                    mark(r as usize, r as usize + 1);
                }
            }
        }
    }
    w
}

/// The head-frame registers the trace tests once, before the loop head:
/// each holds a table on entry, is the table of some store, and is written
/// by no op. Empty when the trace runs host code.
pub(super) fn readonly_invariants(pl: &Plan<'_>, entry_kinds: &[RegKind]) -> Vec<bool> {
    let Plan {
        record,
        effective_end,
        max_stack,
        window_size_us,
        ..
    } = *pl;
    let mut inv = vec![false; window_size_us];
    if record.ops.iter().any(|rop| runs_host_code(rop.inst.op())) {
        return inv;
    }
    let written = written_registers(pl);
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        if !matches!(rop.inst.op(), Op::SetField | Op::SetI | Op::SetTable) {
            continue;
        }
        let r = pl.op_offsets[i] as usize + rop.inst.a() as usize;
        if r < max_stack && !written[r] && entry_kinds.get(r) == Some(&RegKind::Table) {
            inv[r] = true;
        }
    }
    inv
}

/// Whether the store at op `i` into the table in absolute register `r`,
/// whose value is `t`, tests the read-only bit itself; records that `t`
/// is tested from here on.
pub(super) fn store_tests_readonly<E: Emit>(lw: &mut Lower<E>, r: usize, t: Value) -> bool {
    if lw.ro_invariant.get(r).copied().unwrap_or(false) || lw.ro_checked.contains(&t) {
        return false;
    }
    lw.ro_checked.push(t);
    true
}

/// After op `op` is lowered: forget the tested tables when it may have run
/// host code.
pub(super) fn readonly_after_op<E: Emit>(lw: &mut Lower<E>, op: Op) {
    if runs_host_code(op) {
        lw.ro_checked.clear();
    }
}

/// Fills the block before the loop head that tests the invariant tables
/// (see [`readonly_invariants`]): a read-only one leaves at the head with
/// the entry kinds before anything has run, and the interpreter raises.
pub(super) fn emit_readonly_precheck<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let Plan {
        record, max_stack, ..
    } = *pl;
    let Lower {
        reg_state,
        trace_fn_sig_ref,
        ro_precheck,
        body_loop,
        ..
    } = *lw;
    let RuntimeHelpers {
        suppress_admit_id, ..
    } = lw.h.rt;
    let Some(block) = ro_precheck else {
        return;
    };
    lw.bcx.switch_to_block(block);
    lw.bcx.seal_block(block);
    if !lw.ro_invariant.contains(&true) {
        lw.bcx
            .ins()
            .jump(lw.step_precheck.unwrap_or(body_loop), &[]);
        return;
    }
    let entry_stored: Vec<Option<Value>> = lw
        .regs_full
        .iter()
        .map(|&v| Some(lw.bcx.use_var(v)))
        .collect();
    let exit_blk = lw.bcx.create_block();
    for r in 0..max_stack {
        if !lw.ro_invariant[r] {
            continue;
        }
        let t = lw.bcx.use_var(lw.regs_full[r]);
        let ok_blk = lw.bcx.create_block();
        array_slot::emit_writable_guard_to(&mut lw.bcx, t, exit_blk, ok_blk);
        lw.bcx.switch_to_block(ok_blk);
        lw.bcx.seal_block(ok_blk);
    }
    lw.bcx
        .ins()
        .jump(lw.step_precheck.unwrap_or(body_loop), &[]);
    lw.bcx.switch_to_block(exit_blk);
    lw.bcx.seal_block(exit_blk);
    let side_box: Box<TCellPtr> = Box::new(TCellPtr::null());
    let tags_idx = lw.per_exit_kinds.len() as u32;
    lw.per_exit_kinds.push((
        record.head_pc,
        lw.current_kinds[..max_stack].to_vec(),
        side_box,
    ));
    emit_tagged_exit(
        &mut lw.bcx,
        suppress_admit_id,
        &lw.regs_full[..max_stack],
        &entry_stored,
        reg_state,
        record.head_pc,
        record.head_pc,
        tags_idx,
        lw.flush_ctx.as_ref(),
        trace_fn_sig_ref,
    );
}
