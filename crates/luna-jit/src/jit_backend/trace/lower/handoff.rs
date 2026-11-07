//! The move from the baseline tier's loop to the optimizing tier's.
//!
//! The optimizing tier compiles the loop only. Its code starts at
//! `loop_entry`, a block nothing in the baseline code branches to, which
//! sets up again what the trace's entry set up for the loop (the
//! collector's count address, an accumulator's string buffer) and goes on
//! at the loop head; the replay reads the registers the loop reads from
//! `reg_state` before it (see `lir::clif`). The baseline code's back edge,
//! once its iteration count reaches `at` and the trace's loop cell holds
//! that code, stores the registers, sets the count back by one, so that
//! the next entry's first back edge moves again, and calls it, returning
//! what it returns. Every entry so runs its first pass in the baseline
//! code and the rest of the loop in the optimizing tier's, with nothing
//! added inside either loop.

use super::*;
use crate::jit_backend::trace::lir::Count;

/// Creates `loop_entry` for a trace whose back edge moves on to the
/// optimizing tier, once everything else is lowered: emitted before, its
/// branch into the loop head would leave the head's code nothing known
/// from the back edges, which the baseline code would then read from the
/// variables' homes (the replay starts from it the values the loop's code
/// took from the back edges: see `LoopEntry::inherit`).
pub(super) fn emit_loop_entry<E: Emit>(lw: &mut Lower<E>) -> Option<()> {
    if !lw.loops {
        return Some(());
    }
    let at = lw.bcx.create_block();
    lw.bcx.switch_to_block(at);
    lw.bcx.seal_block(at);
    if let Some(var) = lw.due_words {
        let w = read_due_words(&mut lw.bcx)?;
        lw.bcx.def_var(var, w);
    }
    if let Some(ctx) = &lw.flush_ctx {
        // the buffer starts again from the accumulator's string, as on entry
        let (buf_var, slot) = (ctx.buf_var, ctx.accum_slot as usize);
        let acquire = lw.bcx.import_func(lw.h.rt.str_buf_acquire_id);
        let extend = lw.bcx.import_func(lw.h.rt.str_buf_extend_id);
        let call = lw.bcx.ins().call(acquire, &[]);
        let buf = lw.bcx.inst_results(call)[0];
        lw.bcx.def_var(buf_var, buf);
        let accum = lw.bcx.use_var(lw.entry.regs_full[slot]);
        lw.bcx.ins().call(extend, &[buf, accum]);
    }
    lw.bcx.ins().jump(lw.blocks.body_loop, &[]);
    lw.loop_entry = Some(at);
    Some(())
}

/// The back edge's way once its count reaches `at`, in the current block:
/// on into the optimizing tier's loop when the trace's loop cell holds it,
/// else out at the loop head, where the dispatcher moves the trace on.
pub(super) fn emit_hot_back_edge<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let Plan {
        record, max_stack, ..
    } = *pl;
    let (reg_state, sig) = (lw.entry.reg_state, lw.entry.trace_fn_sig_ref);
    if let Some(t) = &lw.tier_count {
        let (count, at) = (count_at(lw, pl, t), t.at());
        let f = load_loop_cell(&mut lw.bcx, t);
        let (go, out) = (lw.bcx.create_block(), lw.bcx.create_block());
        lw.bcx.ins().brif(f, go, &[], out, &[]);
        lw.bcx.switch_to_block(go);
        lw.bcx.seal_block(go);
        mark_pass_into_loop(lw, pl, count, at);
        emit_flush_into_loop(lw, pl);
        for (idx, &v) in lw.entry.regs_full.iter().enumerate() {
            let val = use_var_resolved(&mut lw.bcx, v);
            if lw.stored.get(idx).copied().flatten() != Some(val) {
                lw.bcx
                    .ins()
                    .store(MemFlagsData::new(), val, reg_state, (idx as i32) * 8);
            }
        }
        let call = lw.bcx.ins().call_indirect(sig, f, &[reg_state]);
        let r = lw.bcx.inst_results(call)[0];
        lw.bcx.ins().return_(&[r]);
        lw.bcx.switch_to_block(out);
        lw.bcx.seal_block(out);
    }
    emit_store_back_and_return_pc(
        &mut lw.bcx,
        &lw.entry.regs_full[..max_stack],
        &lw.stored,
        reg_state,
        record.head_pc,
        lw.flush_ctx.as_ref(),
    );
}

/// The optimizing tier's loop of the trace `t` moves, null until compiled.
pub(super) fn load_loop_cell<E: Emit>(bcx: &mut E, t: &TierUp) -> Value {
    let cell_addr = luna_core::__private::tier_up_loop_cell(t) as *const TCellPtr as i64;
    let cell = bcx.reloc(RelocKind::TierLoop, cell_addr);
    bcx.ins().load(types::I64, MemFlagsData::trusted(), cell, 0)
}

/// Before the hand-over to the optimizing tier's loop: the accumulated
/// string made. One the helper leaves to the dispatcher (0 in the register)
/// goes no further: the trace leaves at the loop head instead, its buffer
/// drained already.
fn emit_flush_into_loop<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let Some(ctx) = &lw.flush_ctx else {
        return;
    };
    emit_flush_buf(&mut lw.bcx, ctx, &lw.entry.regs_full);
    let s = lw.bcx.use_var(lw.entry.regs_full[ctx.accum_slot as usize]);
    let (made, unmade) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(s, made, &[], unmade, &[]);
    lw.bcx.switch_to_block(unmade);
    lw.bcx.seal_block(unmade);
    emit_store_back_and_return_pc(
        &mut lw.bcx,
        &lw.entry.regs_full[..pl.max_stack],
        &lw.stored,
        lw.entry.reg_state,
        pl.record.head_pc,
        None,
    );
    lw.bcx.switch_to_block(made);
    lw.bcx.seal_block(made);
}

/// Where the back edge counts: in this call's word of the register buffer
/// when the trace has written-only registers, whose exits tell a call
/// before its first back edge from one past it (see
/// `luna_core::__private::trace_count_slot`), else in the count cell the
/// trace's calls share.
pub(super) fn count_at<E: Emit>(lw: &Lower<E>, pl: &Plan<'_>, t: &TierUp) -> Count {
    if pl.write_only.iter().any(|w| *w) {
        let off = 8 * luna_core::__private::trace_count_slot(pl.window_size_us) as i32;
        Count::Frame(lw.entry.reg_state, off)
    } else {
        Count::Cell(t.count() as *const TCellU32 as i64)
    }
}

/// Before the hand-over to the optimizing tier's loop: the count set back
/// to `at - 1`, and for a count in this call's word its mark of a pass into
/// the loop set.
pub(super) fn mark_pass_into_loop<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    count: Count,
    at: u32,
) {
    let back = lw
        .bcx
        .ins()
        .iconst(types::I32, i64::from(at.saturating_sub(1)));
    let flags = MemFlagsData::trusted();
    let (reg_state, off) = match count {
        Count::Cell(cell) => {
            let count = lw.bcx.reloc(RelocKind::TierCell, cell);
            lw.bcx.ins().store(flags, back, count, 0);
            return;
        }
        Count::Frame(reg_state, off) => (reg_state, off),
    };
    lw.bcx.ins().store(flags, back, reg_state, off);
    let window = pl.window_size_us;
    let one = lw.bcx.ins().iconst(types::I64, 1);
    let passed_off = 8 * luna_core::__private::trace_passed_slot(window) as i32;
    lw.bcx
        .ins()
        .store(MemFlagsData::trusted(), one, reg_state, passed_off);
}
