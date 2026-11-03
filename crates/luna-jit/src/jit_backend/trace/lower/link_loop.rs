//! A loop edge inside a side trace linked to the compiled trace it reached
//! (`TraceRecord::link_pc`): the trace does not end there but goes on the
//! way the recording went, and leaves for the interpreter when the loop
//! goes the other way.

use super::*;

/// The pc the recording went on to after op `i`.
fn next_pc(record: &TraceRecord, i: usize) -> Option<u32> {
    record.ops.get(i + 1).map(|r| r.pc).or(record.link_pc)
}

/// A numeric `for` loop's `ForLoop` at op `oc.i`, in the trace's own frame.
/// The recording either went back into the loop body (the loop goes on:
/// it leaves at `forloop.pc + 1`, the registers untouched, when the loop
/// ends) or past the loop (the loop ends: it leaves at the body, the index
/// stepped, when the loop goes on), as the interpreter runs the op.
pub(super) fn emit_mid_for_loop<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let rop = oc.rop;
    if rop.inline_depth != 0 || pl.record.link_pc.is_none() {
        checkpoint("bail:mid-for-loop");
        return None;
    }
    let body_pc = ((rop.pc as i32) + 1 - rop.inst.bx() as i32).max(0) as u32;
    let next = next_pc(pl.record, oc.i)?;
    let went_on = next == body_pc;
    if !went_on && next != rop.pc + 1 {
        checkpoint("bail:mid-for-loop-next");
        return None;
    }
    let fs = for_step(lw, pl, rop, rop.inst.a() as usize, false)?;
    let (on, off) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(fs.cond, on, &[], off, &[]);
    let saved = fs.r_var.map(|r| (r, lw.current_kinds[r]));
    // the side not recorded leaves first, then the trace goes on along the
    // recorded one
    let (leave, stay) = if went_on { (off, on) } else { (on, off) };
    lw.bcx.switch_to_block(leave);
    lw.bcx.seal_block(leave);
    if went_on {
        guard_exit(lw, pl, rop.pc + 1, oc.i);
    } else {
        step_for(lw, &fs);
        if let Some((r, _)) = saved {
            lw.current_kinds[r] = fs.index_kind;
        }
        guard_exit(lw, pl, body_pc, oc.i);
        if let Some((r, k)) = saved {
            lw.current_kinds[r] = k;
        }
    }
    lw.bcx.switch_to_block(stay);
    lw.bcx.seal_block(stay);
    if went_on {
        step_for(lw, &fs);
        if let Some(r) = fs.r_var {
            lw.current_kinds[r] = fs.index_kind;
        }
    }
    Some(())
}
