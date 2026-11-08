//! The relink block a trace's code goes through after a side trace ended
//! at its head, on to the reentry block (see `side_call`), and the gate
//! after the checks before the loop.

use super::relinks::{Ctx, Gate, Relinks, Way, relinks_of};
use super::side_call::{cold_block, emit_call_block, switch_sealed};
use super::*;
use luna_core::__private::{EXIT_LOOP_AGAIN, trace_ran_side_slot};

/// The way back into a trace that relinks, once everything else is lowered
/// (see [`emit_relinks`]).
pub(super) fn emit_relink_block<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let Some(mut o) = lw.relink.take() else {
        return;
    };
    let x = relinks_of(lw, pl, &o);
    emit_relinks(&mut lw.bcx, &x, &mut o.way);
}

/// The call block, the relink block and the gate of the trace `x`
/// describes, its exits emitted: the relink block counts the loop's
/// iteration and goes on at the reentry block, which redoes what follows
/// the entry block (the registers read from `reg_state`, the collection
/// words, the accumulator, the checks before the loop) and is then sealed.
pub(crate) fn emit_relinks<E: Emit>(bcx: &mut E, x: &Relinks, way: &mut Way) {
    emit_call_block(bcx, x, way);
    if let Some(block) = way.relink {
        switch_sealed(bcx, block);
        let ran = 8 * trace_ran_side_slot(x.window as usize) as i32;
        bcx.mark_relink(block, x.head_pc, ran);
        let cx = x.ctx(bcx);
        count_relink(bcx, x, &cx, ran);
    }
    bcx.seal_block(x.reentry);
    fill_gate(bcx, x);
}

/// Whether the side trace may go on in the optimizing tier's loop taken
/// from the word at `reg_state + ran`, where it was left for the relink
/// block, and the word cleared for the dispatcher; then the loop's iteration
/// counted as at the back edge: reached, `hot` is 1, else 0. On at the
/// reentry block. Only `reg_state` is live here: the baseline tier's code
/// enters the block as a function of its own (see `super::super::lir::Relink`).
fn count_relink<E: Emit>(bcx: &mut E, x: &Relinks, cx: &Ctx<'_>, ran: i32) {
    let (reg_state, flags) = (x.reg_state, MemFlagsData::trusted());
    let ok = bcx.ins().load(types::I64, flags, reg_state, ran);
    bcx.def_var(x.loop_ok, ok);
    let z = bcx.ins().iconst(types::I64, 0);
    bcx.ins().store(flags, z, reg_state, ran);
    bcx.def_var(x.hot, z);
    let Some(t) = cx.tier else {
        bcx.ins().jump(x.reentry, &[]);
        return;
    };
    let hot = cold_block(bcx);
    bcx.tier_count(t.count, t.at, hot, x.reentry);
    switch_sealed(bcx, hot);
    let one = bcx.ins().iconst(types::I64, 1);
    bcx.def_var(x.hot, one);
    bcx.ins().jump(x.reentry, &[]);
}

/// The gate between the checks before the loop and the loop: once the
/// relink block found the loop's count reached, on in the optimizing tier's
/// loop when the side trace left the registers of the kinds that loop is
/// compiled for; otherwise on.
///
/// In the optimizing tier's loop function, which counts nothing, the gate
/// has the function called again when the side trace left the registers
/// of the loop's kinds, and leaves at the loop head for the dispatcher
/// otherwise (see [`loop_fn_gate`]).
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
///
/// What `reg_state` holds is not known in the gate: every register is
/// stored.
fn fill_gate<E: Emit>(bcx: &mut E, x: &Relinks) {
    let Some(g) = x.gate else {
        return;
    };
    switch_sealed(bcx, g.block);
    if !x.loops {
        bcx.ins().jump(g.start, &[]);
        return;
    }
    let cx = x.ctx(bcx);
    let (in_loop_fn, other) = (bcx.create_block(), bcx.create_block());
    bcx.pick(in_loop_fn, other);
    switch_sealed(bcx, in_loop_fn);
    loop_fn_gate(bcx, x, &cx, &g);
    switch_sealed(bcx, other);
    let hot = bcx.use_var(x.hot);
    let go = bcx.create_block();
    bcx.ins().brif(hot, go, &[], g.start, &[]);
    switch_sealed(bcx, go);
    let ok = bcx.use_var(x.loop_ok);
    let (into_loop, other) = (bcx.create_block(), bcx.create_block());
    bcx.ins().brif(ok, into_loop, &[], other, &[]);
    switch_sealed(bcx, other);
    emit_other_kinds(bcx, &cx, g.start);
    switch_sealed(bcx, into_loop);
    emit_hot_back_edge(bcx, &cx, &[]);
}

/// The gate in the optimizing tier's loop function: the registers stored,
/// [`EXIT_LOOP_AGAIN`] returned, for the back edge that called the function
/// to call it again, when the side trace left the registers of the loop's
/// kinds (as it always does without a peeled pass, the loop's kinds being
/// the entry's); else out at the loop head, where the dispatcher enters the
/// trace again. Going on at the loop head here instead would make every
/// register the loop only reads a value the head takes from its branches,
/// which the loop then carries round.
fn loop_fn_gate<E: Emit>(bcx: &mut E, x: &Relinks, cx: &Ctx<'_>, g: &Gate) {
    let again = bcx.ins().iconst(types::I64, EXIT_LOOP_AGAIN as i64);
    let ret = match g.peeled {
        true => {
            let ok = bcx.use_var(x.loop_ok);
            let head = bcx.ins().iconst(types::I64, i64::from(cx.head_pc));
            bcx.ins().select(ok, again, head)
        }
        false => again,
    };
    emit_store_back(bcx, &cx.regs[..cx.max_stack], &[], cx.reg_state, cx.flush);
    bcx.ins().return_(&[ret]);
}

/// The gate's way once the count is reached with registers of other kinds
/// than the optimizing tier's loop takes (see [`fill_gate`]).
fn emit_other_kinds<E: Emit>(bcx: &mut E, cx: &Ctx<'_>, start: Block) {
    let t = cx.tier.expect("a hot loop counts");
    let looped = load_loop_cell(bcx, &t);
    let (keep, leave) = (bcx.create_block(), bcx.create_block());
    bcx.ins().brif(looped, keep, &[], leave, &[]);
    switch_sealed(bcx, leave);
    emit_store_back_and_return_pc(
        bcx,
        &cx.regs[..cx.max_stack],
        &[],
        cx.reg_state,
        cx.head_pc,
        cx.flush,
    );
    switch_sealed(bcx, keep);
    mark_pass_into_loop(bcx, cx, &t);
    bcx.ins().jump(start, &[]);
}
