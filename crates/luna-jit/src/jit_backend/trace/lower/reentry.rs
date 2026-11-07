//! The relink block a trace's code enters itself again at after a side
//! trace ended at its head (see `side_call`), and the gate after the checks
//! before the loop.

use super::side_call::{Relink, switch_sealed};
use super::*;

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
