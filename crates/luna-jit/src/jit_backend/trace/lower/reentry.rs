//! The relink block a trace's code goes through after a side trace ended
//! at its head, on to the reentry block (see `side_call`), and the gate
//! after the checks before the loop.

use super::side_call::{Relink, cold_block, switch_sealed};
use super::*;

/// The relink block, once everything else is lowered: the loop's iteration
/// counted, on at the reentry block, which redoes what follows the entry
/// block (the registers read from `reg_state`, the collection words, the
/// accumulator, the checks before the loop) and is then sealed.
pub(super) fn emit_relink_block<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    super::side_call::emit_call_block(lw, pl);
    let Some(rl) = lw.relink.take() else {
        return;
    };
    if let Some(block) = rl.block {
        switch_sealed(lw, block);
        count_relink(lw, pl, &rl);
    }
    lw.bcx.seal_block(rl.reentry);
    fill_gate(lw, pl, &rl);
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
    let hot = cold_block(lw);
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
