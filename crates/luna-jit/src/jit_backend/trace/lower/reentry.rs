//! The relink block a trace's code goes through after a side trace ended
//! at its head, on to the reentry block (see `side_call`), and the gate
//! after the checks before the loop.

use super::relinks::{Ctx, Gate, Relinks, Way, relinks_of};
use super::side_call::{cold_block, emit_call_block, switch_sealed};
use super::*;
use luna_core::__private::{EXIT_LOOP_AGAIN, trace_ran_side_slot};

/// The way back into a trace that relinks, once everything else is lowered
/// (see [`emit_relinks`]). When it is left to the expansion, the gate goes
/// on to the loop, and what the expansion takes is returned, for a trace
/// with calling exits.
pub(super) fn emit_relink_block<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) -> Option<Relinks> {
    let mut o = lw.relink.take()?;
    let x = relinks_of(lw, pl, &o);
    if !o.defer {
        emit_relinks(&mut lw.bcx, &x, &mut o.way);
        return None;
    }
    if let Some(g) = x.gate {
        switch_sealed(&mut lw.bcx, g.block);
        lw.bcx.ins().jump(g.start, &[]);
    }
    lw.bcx.seal_block(x.reentry);
    (!x.exits.is_empty()).then_some(x)
}

/// The calls of the trace `x` describes, made in `l`, the trace's
/// instructions as lowered with them left to the expansion and made ready
/// to take more (see `super::super::lir::expand`): the two variables the
/// relink block sets cleared on entry, each calling exit on to the call
/// block (or its own, with its cell's plan in `plans`), then the call
/// block, the relink block and the gate.
pub(in crate::jit_backend::trace) fn expand_calls(
    l: &mut lir::Lir,
    x: &Relinks,
    plans: &[Option<std::sync::Arc<luna_core::__private::CellPlan>>],
) {
    let jump = l.reopen(x.entry);
    let z = l.ins().iconst(types::I64, 0);
    l.def_var(x.loop_ok, z);
    l.def_var(x.hot, z);
    l.close(jump);
    let mut way = Way::default();
    for (n, (&b, &tags_idx)) in x.exits.iter().zip(x.cells.iter()).enumerate() {
        let ret = l.reopen(b);
        let ret = l.returned(b, ret);
        match plans.get(n).and_then(Option::as_ref) {
            Some(p) => {
                super::side_plan::emit_planned_exit(l, x, &mut way, n as u32, (ret, tags_idx), p)
            }
            None => super::side_call::emit_exit_call(l, &mut way, n as u32, ret, tags_idx),
        }
    }
    if let Some(g) = x.gate {
        l.reopen(g.block);
    }
    emit_relinks(l, x, &mut way);
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

/// The word at `reg_state + ran`, where the call left it for the relink
/// block (whether the side trace may go on in the optimizing tier's loop,
/// and its exit's note; see [`relinked_ok`]), taken into `loop_ok`, and the
/// word cleared for the dispatcher; then the loop's iteration
/// counted as at the back edge: reached, `hot` is 1, else 0. On at the
/// reentry block. Only `reg_state` is live here: the baseline tier's code
/// enters the block as a function of its own (see `super::super::lir::Relink`).
fn count_relink<E: Emit>(bcx: &mut E, x: &Relinks, cx: &Ctx<'_>, ran: i32) {
    let (reg_state, flags) = (x.reg_state, MemFlagsData::trusted());
    let word = bcx.ins().load(types::I64, flags, reg_state, ran);
    bcx.def_var(x.loop_ok, word);
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
    let ok = relinked_ok(bcx, x);
    let (into_loop, other) = (bcx.create_block(), bcx.create_block());
    bcx.ins().brif(ok, into_loop, &[], other, &[]);
    switch_sealed(bcx, other);
    emit_other_kinds(bcx, x, &cx, g.start);
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
            let ok = relinked_ok(bcx, x);
            let head = bcx.ins().iconst(types::I64, i64::from(cx.head_pc));
            let z = bcx.ins().iconst(types::I64, 0);
            let noted = relinked_note(bcx, x);
            let noted = bcx.ins().select(ok, z, noted);
            note_for_dispatcher(bcx, cx, noted);
            bcx.ins().select(ok, again, head)
        }
        false => again,
    };
    emit_store_back(bcx, &cx.regs[..cx.max_stack], &[], cx.reg_state, cx.flush);
    bcx.ins().return_(&[ret]);
}

/// The gate's way once the count is reached with registers of other kinds
/// than the optimizing tier's loop takes (see [`fill_gate`]).
fn emit_other_kinds<E: Emit>(bcx: &mut E, x: &Relinks, cx: &Ctx<'_>, start: Block) {
    let t = cx.tier.expect("a hot loop counts");
    let looped = load_loop_cell(bcx, &t);
    let (keep, leave) = (bcx.create_block(), bcx.create_block());
    bcx.ins().brif(looped, keep, &[], leave, &[]);
    switch_sealed(bcx, leave);
    let noted = relinked_note(bcx, x);
    note_for_dispatcher(bcx, cx, noted);
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

/// Whether the side trace that relinked may go on in the optimizing tier's
/// loop (see [`count_relink`]).
fn relinked_ok<E: Emit>(bcx: &mut E, x: &Relinks) -> Value {
    let word = bcx.use_var(x.loop_ok);
    bcx.ins().band_imm_u(word, 1)
}

/// The note of the exit whose side trace relinked (see [`count_relink`]).
fn relinked_note<E: Emit>(bcx: &mut E, x: &Relinks) -> Value {
    let word = bcx.use_var(x.loop_ok);
    bcx.ins().ushr_imm_u(word, 32)
}

/// `noted` left in the word the dispatcher reads a side trace's note from:
/// a return at the loop head straight after a relink leaves the registers
/// as the side trace left them, which only its exits' kinds describe, so
/// the dispatcher decodes it as the side trace's own return to the head.
fn note_for_dispatcher<E: Emit>(bcx: &mut E, cx: &Ctx<'_>, noted: Value) {
    let ran = 8 * trace_ran_side_slot(cx.window) as i32;
    bcx.ins()
        .store(MemFlagsData::trusted(), noted, cx.reg_state, ran);
}
