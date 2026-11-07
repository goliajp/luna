//! A trace's exits that run the side trace wired to them from the trace's
//! own code, and the way back into the trace when that side trace ends at
//! the trace's head (see `luna_core::__private::SideCell`).
//!
//! Such an exit stores the registers as any exit does, then reads its side
//! cell. Empty, the exit returns as usual and the dispatcher takes over.
//! Holding an entry, the exit counts the side trace's move to the
//! optimizing tier (leaving it to the dispatcher once the count is
//! reached), checks what the dispatcher checks before it runs a side trace
//! (leaving the side trace to it when a check fails), and calls it on the
//! same registers. A side trace that returns
//! [`EXIT_RELINK`] has ended at this trace's head: its stores to the stack
//! made, the trace enters itself again at the relink block, which does what
//! the entry block and the dispatcher do before a call, and goes on where
//! the entry block does, ahead of the checks before the loop. Any other
//! return is returned, the exit noted for the dispatcher to decode it with
//! the side trace's exits.
//!
//! The relink block counts an iteration of the loop, as the back edge
//! does; once the count is reached, and the side trace leaves the
//! registers of the kinds the loop is compiled for, the gate after the
//! checks goes on in the optimizing tier's loop.

use super::*;
use luna_core::__private::{EXIT_RELINK, SideCell, trace_ran_side_slot};

/// What the trace's way back in needs, for a trace whose exits may run a
/// side trace from its code.
pub(super) struct Relink {
    /// Where the entry block goes on: sealed once every relink block
    /// branches to it.
    pub(super) reentry: Block,
    /// The relink block, once an exit branches to it.
    pub(super) block: Option<Block>,
    /// Set by the exit that relinks: whether the side trace's registers
    /// may go on in the optimizing tier's loop.
    pub(super) loop_ok: Variable,
    /// Set by the relink block: the loop's count is reached and the gate
    /// goes on in the optimizing tier's loop.
    pub(super) hot: Variable,
}

/// Whether the trace's exits may run side traces from its code: a root
/// trace closing by its loop. A side trace's exits run none (it has no
/// side traces of its own), nor do AOT code's, whose cells no loader fills.
pub(super) fn relinks(pl: &Plan<'_>) -> bool {
    pl.record.side_trace_parent.is_none() && pl.downrec_idx_opt.is_none() && !pl.opts.aot
}

/// In the entry block, once it is done: the reentry block the entry and
/// the relink blocks go on at, made current (unsealed).
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
    bcx.ins().jump(reentry, &[]);
    bcx.switch_to_block(reentry);
    Some(Relink {
        reentry,
        block: None,
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
    emit_store_back(
        &mut lw.bcx,
        &lw.entry.regs_full[..pl.max_stack],
        &lw.stored,
        lw.entry.reg_state,
        lw.flush_ctx.as_ref(),
    );
    let cell = Box::new(SideCell::new());
    let n = lw.side_cells.len() as u32;
    let c = lw
        .bcx
        .reloc(RelocKind::SideCell(n), &*cell as *const SideCell as i64);
    lw.side_cells.push((tags_idx, cell));
    let out = lw.bcx.create_block();
    let entry = emit_may_call(lw, c, out);
    emit_call(lw, pl, c, entry, tags_idx);
    switch_sealed(lw, out);
    let ret =
        luna_core::__private::EXIT_TAGS_INDEX_BIT | (u64::from(tags_idx) << 32) | u64::from(pc);
    let ret = lw.bcx.ins().iconst(types::I64, ret as i64);
    lw.bcx.ins().return_(&[ret]);
}

/// The side trace's entry in cell `c`, once the code may call it: on to
/// `out` when the cell is empty, the side trace's move is due (counted
/// here; the dispatcher makes it) or one of the dispatcher's checks before
/// it runs a side trace fails.
fn emit_may_call<E: Emit>(lw: &mut Lower<E>, c: Value, out: Block) -> Value {
    let (at, flags) = (SideCell::offsets(), MemFlagsData::trusted());
    let entry = lw.bcx.ins().load(types::I64, flags, c, at.entry);
    let count_blk = lw.bcx.create_block();
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
    let check_blk = lw.bcx.create_block();
    lw.bcx.ins().brif(reached, out, &[], check_blk, &[]);
    switch_sealed(lw, check_blk);
    // the exit left nothing for the dispatcher to do (a deopt, a collection
    // check, something to make), and the slots the side trace reads that
    // this trace does not check hold the tags it reads them as
    let call_blk = lw.bcx.create_block();
    let check = lw.bcx.ins().load(types::I64, flags, c, at.check);
    let f = lw.bcx.import_func(lw.h.rt.side_cell_list_id);
    let ok = lw.bcx.ins().call(f, &[check, lw.entry.reg_state]);
    let ok = lw.bcx.inst_results(ok)[0];
    lw.bcx.ins().brif(ok, call_blk, &[], out, &[]);
    switch_sealed(lw, call_blk);
    entry
}

/// The call of the side trace at `entry` (cell `c`'s), from the exit to
/// typed exit `tags_idx`: on at the relink block when it goes on in this
/// trace's loop, else its return returned, the exit noted.
fn emit_call<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, c: Value, entry: Value, tags_idx: u32) {
    let (reg_state, flags) = (lw.entry.reg_state, MemFlagsData::trusted());
    let sig = lw.entry.trace_fn_sig_ref;
    let call = lw.bcx.ins().call_indirect(sig, entry, &[reg_state]);
    let r = lw.bcx.inst_results(call)[0];
    let relinked = lw.bcx.ins().band_imm_u(r, EXIT_RELINK as i64);
    let (back_blk, other) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(relinked, back_blk, &[], other, &[]);
    switch_sealed(lw, other);
    let noted = lw.bcx.ins().iconst(types::I64, i64::from(tags_idx) + 1);
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
    let rl = lw.relink.as_mut().expect("a calling exit has a way back");
    let (loop_ok, block) = (
        rl.loop_ok,
        *rl.block.get_or_insert_with(|| lw.bcx.create_block()),
    );
    lw.bcx.def_var(loop_ok, ok);
    lw.bcx.ins().jump(block, &[]);
}

fn switch_sealed<E: Emit>(lw: &mut Lower<E>, b: Block) {
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
    let (store_blk, done) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(back, store_blk, &[], done, &[]);
    switch_sealed(lw, store_blk);
    let f = lw.bcx.import_func(lw.h.rt.side_cell_list_id);
    lw.bcx.ins().call(f, &[back, lw.entry.reg_state]);
    lw.bcx.ins().jump(done, &[]);
    switch_sealed(lw, done);
}

/// The relink block, once everything else is lowered: what the entry block
/// and the dispatcher do before a call, the loop's iteration counted, on at
/// the reentry block, which is then sealed.
pub(super) fn emit_relink_block<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) -> Option<()> {
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

/// The loop's iteration counted as at the back edge; reached, `hot` is the
/// side trace's `loop_ok`, else 0. On at the reentry block.
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
    let ok = lw.bcx.use_var(rl.loop_ok);
    lw.bcx.def_var(rl.hot, ok);
    lw.bcx.ins().jump(rl.reentry, &[]);
}

/// The gate between the checks before the loop and the loop: on in the
/// optimizing tier's loop when the relink block found it hot, else on.
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
    emit_hot_back_edge(lw, pl);
    lw.stored = stored;
}
