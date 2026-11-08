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

use super::*;
use cranelift_codegen::ir::BlockArg;
use luna_core::__private::{SideCell, SideCells, side_cells, trace_ran_side_slot};
use std::sync::Arc;

/// What the trace's way back in needs, for a trace whose exits may run a
/// side trace from its code.
pub(super) struct Relink {
    /// Where the entry block goes on: sealed once every relink block
    /// branches to it.
    pub(super) reentry: Block,
    /// The relink block, once an exit branches to it.
    pub(super) block: Option<Block>,
    /// The call block every calling exit goes on at, once one does: its
    /// parameters are the exit's side cell, its return, and the word the
    /// dispatcher reads to know the side trace ran (see [`emit_call_block`]).
    pub(super) call: Option<Block>,
    /// Set by the exit that relinks: whether the side trace's registers
    /// may go on in the optimizing tier's loop.
    pub(super) loop_ok: Variable,
    /// Set by the relink block: the loop's count is reached, and the gate
    /// moves on to the optimizing tier.
    pub(super) hot: Variable,
}

/// Whether the trace's exits may run side traces from its code: a root
/// trace closing by its loop. A side trace's exits run none (it has no
/// side traces of its own), nor do AOT code's, whose cells no loader fills.
pub(super) fn relinks(pl: &Plan<'_>) -> bool {
    pl.record.side_trace_parent.is_none() && pl.downrec_idx_opt.is_none() && !pl.opts.aot
}

/// In the entry block, right after its parameter: the reentry block the
/// entry and the relink blocks go on at, made current (unsealed).
pub(super) fn open_reentry<E: Emit>(bcx: &mut E, pl: &Plan<'_>) -> Option<Relink> {
    if !relinks(pl) {
        return None;
    }
    let loop_ok = bcx.declare_var(types::I64);
    let hot = bcx.declare_var(types::I64);
    let z = bcx.ins().iconst(types::I64, 0);
    bcx.def_var(loop_ok, z);
    bcx.def_var(hot, z);
    let reentry = bcx.create_block();
    bcx.mark_reentry(reentry);
    bcx.ins().jump(reentry, &[]);
    bcx.switch_to_block(reentry);
    Some(Relink {
        reentry,
        block: None,
        call: None,
        loop_ok,
        hot,
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
    let c = cell_at(&mut lw.bcx, n);
    lw.side_cells.push(tags_idx);
    let ret =
        luna_core::__private::EXIT_TAGS_INDEX_BIT | (u64::from(tags_idx) << 32) | u64::from(pc);
    let ret = lw.bcx.ins().iconst(types::I64, ret as i64);
    let noted = lw.bcx.ins().iconst(types::I64, i64::from(tags_idx) + 1);
    let rl = lw.relink.as_mut().expect("a calling exit has a way back");
    let call = match rl.call {
        Some(b) => b,
        None => {
            let b = lw.bcx.create_block();
            lw.bcx.set_cold_block(b);
            for _ in 0..3 {
                lw.bcx.append_block_param(b, types::I64);
            }
            rl.call = Some(b);
            b
        }
    };
    let args = [c, ret, noted].map(BlockArg::Value);
    lw.bcx.calling_exit(n, call, &args);
}

/// The address of side cell `n`: the trace's first cell's, written in by
/// `place_cells` once every cell is known, and `n` cells past it.
pub(super) fn cell_at<E: Emit>(bcx: &mut E, n: u32) -> Value {
    let first = bcx.reloc(RelocKind::SideCells, 0);
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

/// The call block, once every exit is lowered: the side trace of the cell
/// it is given called when the cell holds one and it may run, else the
/// exit's return returned.
pub(super) fn emit_call_block<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let Some(call) = lw.relink.as_ref().and_then(|rl| rl.call) else {
        return;
    };
    switch_sealed(lw, call);
    let &[c, ret, noted] = lw.bcx.block_params(call) else {
        unreachable!("the call block takes a cell, a return and a note")
    };
    let out = cold_block(lw);
    let entry = emit_may_call(lw, c, out);
    emit_call(lw, pl, c, entry, noted);
    switch_sealed(lw, out);
    lw.bcx.ins().return_(&[ret]);
}

/// The side trace's entry in cell `c`, once the code may call it: on to
/// `out` when the cell is empty, the side trace's move is due (counted
/// here; the dispatcher makes it) or one of the dispatcher's checks before
/// it runs a side trace fails.
fn emit_may_call<E: Emit>(lw: &mut Lower<E>, c: Value, out: Block) -> Value {
    let (at, flags) = (SideCell::offsets(), MemFlagsData::trusted());
    let entry = lw.bcx.ins().load(types::I64, flags, c, at.entry);
    let count_blk = cold_block(lw);
    lw.bcx.ins().brif(entry, count_blk, &[], out, &[]);
    switch_sealed(lw, count_blk);
    let cp = lw.bcx.ins().load(types::I64, flags, c, at.count);
    let n0 = lw.bcx.ins().load(types::I32, flags, cp, 0);
    let n1 = lw.bcx.ins().iadd_imm_s(n0, 1);
    lw.bcx.ins().store(flags, n1, cp, 0);
    let limit = lw.bcx.ins().load(types::I32, flags, c, at.at);
    let reached = lw
        .bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, n1, limit);
    let check_blk = cold_block(lw);
    lw.bcx.ins().brif(reached, out, &[], check_blk, &[]);
    switch_sealed(lw, check_blk);
    // the exit left nothing for the dispatcher to do (a deopt, a collection
    // check, something to make), and the slots the side trace reads that
    // this trace does not check hold the tags it reads them as
    let call_blk = cold_block(lw);
    let check = lw.bcx.ins().load(types::I64, flags, c, at.check);
    let f = lw.bcx.import_func(lw.h.rt.side_cell_list_id);
    let ok = lw.bcx.ins().call(f, &[check, lw.entry.reg_state]);
    let ok = lw.bcx.inst_results(ok)[0];
    lw.bcx.ins().brif(ok, call_blk, &[], out, &[]);
    switch_sealed(lw, call_blk);
    entry
}

/// The call of the side trace at `entry` (cell `c`'s): on at the relink
/// block when it goes on in this trace's loop, else its return returned,
/// `noted` (the typed exit + 1) stored for the dispatcher.
fn emit_call<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, c: Value, entry: Value, noted: Value) {
    let (reg_state, flags) = (lw.entry.reg_state, MemFlagsData::trusted());
    let sig = lw.entry.trace_fn_sig_ref;
    let call = lw.bcx.ins().call_indirect(sig, entry, &[reg_state]);
    let r = lw.bcx.inst_results(call)[0];
    let (back_blk, other) = (cold_block(lw), cold_block(lw));
    super::side_link::emit_branch_on_return(lw, pl, c, r, back_blk, other);
    switch_sealed(lw, other);
    let slot = 8 * trace_ran_side_slot(pl.window_size_us) as i32;
    lw.bcx.ins().store(flags, noted, reg_state, slot);
    lw.bcx.ins().return_(&[r]);
    switch_sealed(lw, back_blk);
    emit_back_stores(lw, c);
    let ok = lw
        .bcx
        .ins()
        .load(types::I32, flags, c, SideCell::offsets().loop_ok);
    let ok = lw.bcx.ins().uextend(types::I64, ok);
    // the relink block takes it from the word, as it does from the
    // baseline tier's call stub (see `reentry::count_relink`)
    lw.bcx.ins().store(flags, ok, reg_state, slot);
    let rl = lw.relink.as_mut().expect("a calling exit has a way back");
    let block = match rl.block {
        Some(b) => b,
        None => {
            let b = lw.bcx.create_block();
            lw.bcx.set_cold_block(b);
            *rl.block.insert(b)
        }
    };
    lw.bcx.ins().jump(block, &[]);
}

/// A new block on the way of a side trace run from the code, which the
/// loop seldom takes.
pub(super) fn cold_block<E: Emit>(lw: &mut Lower<E>) -> Block {
    let b = lw.bcx.create_block();
    lw.bcx.set_cold_block(b);
    b
}

pub(super) fn switch_sealed<E: Emit>(lw: &mut Lower<E>, b: Block) {
    lw.bcx.switch_to_block(b);
    lw.bcx.seal_block(b);
}

/// The relink count of cell `c` bumped, and its stores to the stack made
/// when it lists any.
fn emit_back_stores<E: Emit>(lw: &mut Lower<E>, c: Value) {
    let (at, flags) = (SideCell::offsets(), MemFlagsData::trusted());
    let k = lw.bcx.ins().load(types::I32, flags, c, at.relinks);
    let k = lw.bcx.ins().iadd_imm_s(k, 1);
    lw.bcx.ins().store(flags, k, c, at.relinks);
    let back = lw.bcx.ins().load(types::I64, flags, c, at.back);
    let (store_blk, done) = (cold_block(lw), cold_block(lw));
    lw.bcx.ins().brif(back, store_blk, &[], done, &[]);
    switch_sealed(lw, store_blk);
    let f = lw.bcx.import_func(lw.h.rt.side_cell_list_id);
    lw.bcx.ins().call(f, &[back, lw.entry.reg_state]);
    lw.bcx.ins().jump(done, &[]);
    switch_sealed(lw, done);
}
