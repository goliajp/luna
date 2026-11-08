//! A trace's exits that run the side trace wired to them from the trace's
//! own code, and the way back into the trace when that side trace ends at
//! the trace's head (see `luna_core::__private::SideCell`).
//!
//! Such an exit stores the registers as any exit does, then goes on at the
//! trace's one call block with its side cell and its return. The call
//! block reads the cell. Empty, it returns the exit's return as usual and
//! the dispatcher takes over. Holding an entry, it counts the side trace's move to the
//! optimizing tier (leaving it to the dispatcher once the count is
//! reached), checks what the dispatcher checks before it runs a side trace
//! (leaving the side trace to it when a check fails), and calls it on the
//! same registers. A side trace that returns
//! [`EXIT_RELINK`] has ended at this trace's head: its stores to the stack
//! made, the trace goes through the relink block to the reentry block,
//! which the entry block also goes on at and which does all the code does
//! once per entry (the registers read from `reg_state`, the checks before
//! the loop), so that only `reg_state` is carried across the call. Any other
//! return is returned, the exit noted for the dispatcher to decode it with
//! the side trace's exits.
//!
//! The relink block counts an iteration of the loop, as the back edge
//! does; once the count is reached, the gate after the checks goes on in
//! the optimizing tier's loop when the side trace leaves the registers of
//! the kinds the loop is compiled for; else, before the optimizing tier's
//! code exists, it leaves at the loop head for the dispatcher to move the
//! trace, and after, it goes on in its own loop with the count set back by
//! one (see `reentry`).
//!
//! The baseline tier's code has no call block: its exits read the cell
//! themselves and go on to a call stub shared by every trace, which does
//! what the call block does and enters the relink block as a function of
//! its own (see `super::super::lir::stub`).
//!
//! Lowered into the instructions the baseline tier compiles, none of this
//! is made until a side trace is first wired to one of the exits: until
//! then each such exit returns as any exit does, and the trace has no
//! cells (see `relinks`).

use super::relinks::{Open, Relinks, Way};
use super::side_plan::{emit_entry_counted, emit_noted_return, emit_relink_word};
use super::*;
use cranelift_codegen::ir::BlockArg;
use luna_core::__private::{SideCell, SideCells, side_cells};
use std::sync::Arc;

/// Whether the trace's exits may run side traces from its code: a root
/// trace closing by its loop. A side trace's exits run none (it has no
/// side traces of its own), nor do AOT code's, whose cells no loader fills.
pub(super) fn relinks(pl: &Plan<'_>) -> bool {
    pl.record.side_trace_parent.is_none() && pl.downrec_idx_opt.is_none() && !pl.opts.aot
}

/// In the entry block, right after its parameter: the reentry block the
/// entry and the relink blocks go on at, made current (unsealed).
/// `defer`: see [`Open::defer`] (the expansion sets the two variables).
pub(super) fn open_reentry<E: Emit>(bcx: &mut E, pl: &Plan<'_>, defer: bool) -> Option<Open> {
    if !relinks(pl) {
        return None;
    }
    let entry = bcx.current_block().expect("the entry block is current");
    let loop_ok = bcx.declare_var(types::I64);
    let hot = bcx.declare_var(types::I64);
    if !defer {
        let z = bcx.ins().iconst(types::I64, 0);
        bcx.def_var(loop_ok, z);
        bcx.def_var(hot, z);
    }
    let reentry = bcx.create_block();
    bcx.mark_reentry(reentry);
    bcx.ins().jump(reentry, &[]);
    bcx.switch_to_block(reentry);
    Some(Open {
        defer,
        entry,
        exits: Vec::new(),
        reentry,
        loop_ok,
        hot,
        way: Way::default(),
    })
}

/// Whether the depth-0 exit at `pc` runs its side trace from the code: not
/// one to the head, whose exit stops the dispatcher re-entering the trace.
pub(super) fn exit_calls<E: Emit>(lw: &Lower<E>, pl: &Plan<'_>, pc: u32) -> bool {
    lw.relink.is_some() && pc != pl.record.head_pc
}

/// A depth-0 exit to `pc` restoring through `per_exit_tags[tags_idx]`
/// that runs its side trace from the code (see the module comment).
pub(super) fn emit_calling_exit<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, pc: u32, tags_idx: u32) {
    if let Some(b) = lw.bcx.current_block() {
        lw.bcx.set_cold_block(b);
    }
    emit_store_back(
        &mut lw.bcx,
        &lw.entry.regs_full[..pl.max_stack],
        &lw.stored,
        lw.entry.reg_state,
        lw.flush_ctx.as_ref(),
    );
    let n = lw.side_cells.len() as u32;
    lw.side_cells.push(tags_idx);
    let ret =
        luna_core::__private::EXIT_TAGS_INDEX_BIT | (u64::from(tags_idx) << 32) | u64::from(pc);
    let ret = lw.bcx.ins().iconst(types::I64, ret as i64);
    let rl = lw.relink.as_mut().expect("a calling exit has a way back");
    if rl.defer {
        rl.exits.extend(lw.bcx.current_block());
        lw.bcx.ins().return_(&[ret]);
        return;
    }
    emit_exit_call(&mut lw.bcx, &mut rl.way, n, ret, tags_idx);
}

/// The calling exit of cell `n`, of typed exit `tags_idx`, returning `ret`:
/// on to the call block with its cell, its return and its note.
pub(crate) fn emit_exit_call<E: Emit>(
    bcx: &mut E,
    way: &mut Way,
    n: u32,
    ret: Value,
    tags_idx: u32,
) {
    let c = cell_at(bcx, n);
    let noted = bcx.ins().iconst(types::I64, i64::from(tags_idx) + 1);
    let call = match way.call {
        Some(b) => b,
        None => {
            let b = cold_block(bcx);
            for _ in 0..3 {
                bcx.append_block_param(b, types::I64);
            }
            *way.call.insert(b)
        }
    };
    let args = [c, ret, noted].map(BlockArg::Value);
    bcx.calling_exit(n, call, &args);
}

/// The address of side cell `n`: the trace's first cell's, and `n` cells
/// past it. The first's is known once the cells are made (when the calls
/// are expanded), else written in by `place_cells`.
pub(crate) fn cell_at<E: Emit>(bcx: &mut E, n: u32) -> Value {
    let known =
        (bcx.relocs_mut().iter()).find_map(|r| (r.0 == RelocKind::SideCells).then_some(r.1));
    let first = bcx.reloc(RelocKind::SideCells, known.unwrap_or(0));
    match n {
        0 => first,
        n => bcx
            .ins()
            .iadd_imm_s(first, i64::from(n) * size_of::<SideCell>() as i64),
    }
}

/// The cells of the exits `exits` (typed exit per cell), allocated at once,
/// with `bcx`'s relocation of the first set to its address.
pub(super) fn place_cells<E: Emit>(
    bcx: &mut E,
    exits: Vec<u32>,
) -> (Arc<[u32]>, Option<SideCells>) {
    if exits.is_empty() {
        return (Arc::new([]), None);
    }
    let cells = side_cells(exits.len());
    for (kind, live) in bcx.relocs_mut() {
        if *kind == RelocKind::SideCells {
            *live = cells.as_ptr() as i64;
        }
    }
    (exits.into(), Some(cells))
}

/// The call block, once every exit is emitted: the side trace of the cell
/// it is given called when the cell holds one and it may run, else the
/// exit's return returned.
pub(super) fn emit_call_block<E: Emit>(bcx: &mut E, x: &Relinks, way: &mut Way) {
    let Some(call) = way.call else {
        return;
    };
    switch_sealed(bcx, call);
    let &[c, ret, noted] = bcx.block_params(call) else {
        unreachable!("the call block takes a cell, a return and a note")
    };
    let out = cold_block(bcx);
    let entry = emit_may_call(bcx, x, c, out);
    emit_call(bcx, x, way, (c, entry, noted));
    switch_sealed(bcx, out);
    bcx.ins().return_(&[ret]);
}

/// The side trace's entry in cell `c`, once the code may call it: on to
/// `out` when the cell is empty, the side trace's move is due (counted
/// here; the dispatcher makes it) or one of the dispatcher's checks before
/// it runs a side trace fails.
fn emit_may_call<E: Emit>(bcx: &mut E, x: &Relinks, c: Value, out: Block) -> Value {
    let entry = emit_entry_counted(bcx, c, out);
    // the exit left nothing for the dispatcher to do (a deopt, a collection
    // check, something to make), and the slots the side trace reads that
    // this trace does not check hold the tags it reads them as
    let call_blk = cold_block(bcx);
    let check = (bcx.ins()).load(
        types::I64,
        MemFlagsData::trusted(),
        c,
        SideCell::offsets().check,
    );
    let ok = bcx.ins().call(x.list_fn, &[check, x.reg_state]);
    let ok = bcx.inst_results(ok)[0];
    bcx.ins().brif(ok, call_blk, &[], out, &[]);
    switch_sealed(bcx, call_blk);
    entry
}

/// The call of the side trace at `entry` (cell `c`'s): on at the relink
/// block when it goes on in this trace's loop, else its return returned,
/// `noted` (the typed exit + 1) stored for the dispatcher.
fn emit_call<E: Emit>(
    bcx: &mut E,
    x: &Relinks,
    way: &mut Way,
    (c, entry, noted): (Value, Value, Value),
) {
    let flags = MemFlagsData::trusted();
    let call = bcx.ins().call_indirect(x.sig, entry, &[x.reg_state]);
    let r = bcx.inst_results(call)[0];
    let (back_blk, other) = (cold_block(bcx), cold_block(bcx));
    super::side_link::emit_branch_on_return(bcx, x, c, r, back_blk, other);
    switch_sealed(bcx, other);
    emit_noted_return(bcx, x, noted, r);
    switch_sealed(bcx, back_blk);
    emit_back_stores(bcx, x, c);
    let ok = bcx
        .ins()
        .load(types::I32, flags, c, SideCell::offsets().loop_ok);
    let ok = bcx.ins().uextend(types::I64, ok);
    emit_relink_word(bcx, x, way, ok, noted);
}

/// A new block on the way of a side trace run from the code, which the
/// loop seldom takes.
pub(super) fn cold_block<E: Emit>(bcx: &mut E) -> Block {
    let b = bcx.create_block();
    bcx.set_cold_block(b);
    b
}

pub(super) fn switch_sealed<E: Emit>(bcx: &mut E, b: Block) {
    bcx.switch_to_block(b);
    bcx.seal_block(b);
}

/// The relink count of cell `c` bumped, and its stores to the stack made
/// when it lists any.
fn emit_back_stores<E: Emit>(bcx: &mut E, x: &Relinks, c: Value) {
    let (at, flags) = (SideCell::offsets(), MemFlagsData::trusted());
    let k = bcx.ins().load(types::I32, flags, c, at.relinks);
    let k = bcx.ins().iadd_imm_s(k, 1);
    bcx.ins().store(flags, k, c, at.relinks);
    let back = bcx.ins().load(types::I64, flags, c, at.back);
    let (store_blk, done) = (cold_block(bcx), cold_block(bcx));
    bcx.ins().brif(back, store_blk, &[], done, &[]);
    switch_sealed(bcx, store_blk);
    bcx.ins().call(x.list_fn, &[back, x.reg_state]);
    bcx.ins().jump(done, &[]);
    switch_sealed(bcx, done);
}
