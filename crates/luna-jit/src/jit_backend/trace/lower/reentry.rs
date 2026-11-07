//! The relink block a trace's code enters itself again at after a side
//! trace ended at its head (see `side_call`), and the gate after the checks
//! before the loop.

use super::side_call::{Relink, switch_sealed};
use super::*;

/// The relink block, once everything else is lowered: what the entry block
/// and the dispatcher do before a call, the loop's iteration counted, on at
/// the reentry block, which is then sealed.
pub(super) fn emit_relink_block<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) -> Option<()> {
    super::side_call::emit_call_block(lw, pl);
    let Some(rl) = lw.relink.take() else {
        return Some(());
    };
    if let Some(block) = rl.block {
        switch_sealed(lw, block);
        reload_registers(lw, pl);
        if let Some(var) = lw.due_words {
            let w = read_due_words(&mut lw.bcx)?;
            lw.bcx.def_var(var, w);
        }
        restart_accum(lw);
        count_relink(lw, pl, &rl);
    }
    lw.bcx.seal_block(rl.reentry);
    fill_gate(lw, pl, &rl);
    Some(())
}

/// The registers as the entry block reads them: the head frame's from
/// `reg_state`, the inline frames' zero (in `reg_state` too).
fn reload_registers<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let reg_state = lw.entry.reg_state;
    for i in 0..pl.window_size_us {
        let var = lw.entry.regs_full[i];
        let v = if i < pl.max_stack {
            lw.bcx
                .ins()
                .load(types::I64, MemFlagsData::new(), reg_state, (i as i32) * 8)
        } else {
            let z = lw.bcx.ins().iconst(types::I64, 0);
            lw.bcx
                .ins()
                .store(MemFlagsData::new(), z, reg_state, (i as i32) * 8);
            z
        };
        lw.bcx.def_var(var, v);
    }
    let z = lw.bcx.ins().iconst(types::I64, 0);
    lw.bcx.def_var(lw.entry.tforcall_tag_var, z);
    lw.bcx.def_var(lw.entry.tforcall_val_tag_var, z);
}

/// The accumulator's buffer started again from its string, as on entry
/// (the exit flushed it before calling the side trace).
fn restart_accum<E: Emit>(lw: &mut Lower<E>) {
    let Some(ctx) = &lw.flush_ctx else {
        return;
    };
    let (buf_var, slot) = (ctx.buf_var, ctx.accum_slot as usize);
    let acquire = lw.bcx.import_func(lw.h.rt.str_buf_acquire_id);
    let extend = lw.bcx.import_func(lw.h.rt.str_buf_extend_id);
    let call = lw.bcx.ins().call(acquire, &[]);
    let buf = lw.bcx.inst_results(call)[0];
    lw.bcx.def_var(buf_var, buf);
    let accum = lw.bcx.use_var(lw.entry.regs_full[slot]);
    lw.bcx.ins().call(extend, &[buf, accum]);
}

/// The loop's iteration counted as at the back edge; reached, `hot` is 1,
/// else 0. On at the reentry block.
fn count_relink<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, rl: &Relink) {
    let z = lw.bcx.ins().iconst(types::I64, 0);
    lw.bcx.def_var(rl.hot, z);
    let Some(t) = lw.tier_count.as_ref().filter(|_| lw.loops) else {
        lw.bcx.ins().jump(rl.reentry, &[]);
        return;
    };
    let (count, at) = (count_at(lw, pl, t), t.at());
    let hot = lw.bcx.create_block();
    lw.bcx.tier_count(count, at, hot, rl.reentry);
    switch_sealed(lw, hot);
    let one = lw.bcx.ins().iconst(types::I64, 1);
    lw.bcx.def_var(rl.hot, one);
    lw.bcx.ins().jump(rl.reentry, &[]);
}

/// The gate between the checks before the loop and the loop: once the
/// relink block found the loop's count reached, on in the optimizing tier's
/// loop when the side trace left the registers of the kinds that loop is
/// compiled for; otherwise on.
///
/// With registers of other kinds, before the optimizing tier's code exists
/// (the baseline code reaches the count first: the dispatcher moves the
/// trace only at an entry), the trace leaves at the loop head, where the
/// dispatcher moves it and enters the optimizing tier's entry, which checks
/// the registers itself. The count is reached once, so the baseline code
/// leaves so at most once. Once that code exists (in the optimizing tier's
/// first pass, whose count is always reached, or in a baseline call the
/// trace moved under), the gate goes on with the count set back by one: the
/// next back edge, past a pass that leaves the registers of the loop's
/// kinds, goes on in the optimizing tier's loop.
fn fill_gate<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, rl: &Relink) {
    let Some(gate) = lw.blocks.gate else {
        return;
    };
    let start = lw.blocks.peel.unwrap_or(lw.blocks.body_loop);
    switch_sealed(lw, gate);
    if !lw.loops {
        lw.bcx.ins().jump(start, &[]);
        return;
    }
    let hot = lw.bcx.use_var(rl.hot);
    let go = lw.bcx.create_block();
    lw.bcx.ins().brif(hot, go, &[], start, &[]);
    switch_sealed(lw, go);
    // what reg_state holds is not known here: every register is stored
    let stored = std::mem::take(&mut lw.stored);
    let ok = lw.bcx.use_var(rl.loop_ok);
    let (into_loop, other) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(ok, into_loop, &[], other, &[]);
    switch_sealed(lw, other);
    emit_other_kinds(lw, pl, start);
    switch_sealed(lw, into_loop);
    emit_hot_back_edge(lw, pl);
    lw.stored = stored;
}

/// The gate's way once the count is reached with registers of other kinds
/// than the optimizing tier's loop takes (see [`fill_gate`]).
fn emit_other_kinds<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, start: Block) {
    let t = lw.tier_count.as_ref().expect("a hot loop counts");
    let (count, at) = (count_at(lw, pl, t), t.at());
    let looped = load_loop_cell(&mut lw.bcx, t);
    let (keep, leave) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(looped, keep, &[], leave, &[]);
    switch_sealed(lw, leave);
    emit_store_back_and_return_pc(
        &mut lw.bcx,
        &lw.entry.regs_full[..pl.max_stack],
        &lw.stored,
        lw.entry.reg_state,
        pl.record.head_pc,
        lw.flush_ctx.as_ref(),
    );
    switch_sealed(lw, keep);
    mark_pass_into_loop(lw, pl, count, at);
    lw.bcx.ins().jump(start, &[]);
}
