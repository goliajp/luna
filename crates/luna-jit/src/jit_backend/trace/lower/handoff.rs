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
    lw.bcx.ins().jump(lw.body_loop, &[]);
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
        let cell_addr = luna_core::__private::tier_up_loop_cell(t) as *const TCellPtr as i64;
        let count_addr = t.count() as *const TCellU32 as i64;
        let at = t.at();
        let cell = lw.bcx.reloc(RelocKind::TierLoop, cell_addr);
        let f = lw
            .bcx
            .ins()
            .load(types::I64, MemFlagsData::trusted(), cell, 0);
        let (go, out) = (lw.bcx.create_block(), lw.bcx.create_block());
        lw.bcx.ins().brif(f, go, &[], out, &[]);
        lw.bcx.switch_to_block(go);
        lw.bcx.seal_block(go);
        let count = lw.bcx.reloc(RelocKind::TierCell, count_addr);
        let back = lw
            .bcx
            .ins()
            .iconst(types::I32, i64::from(at.saturating_sub(1)));
        lw.bcx.ins().store(MemFlagsData::trusted(), back, count, 0);
        if let Some(ctx) = &lw.flush_ctx {
            emit_flush_buf(&mut lw.bcx, ctx, &lw.entry.regs_full);
        }
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
