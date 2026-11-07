//! The return of a side trace a trace's code ran from a side cell: on in
//! the trace's own loop when it ended at the trace's head, into the trace
//! it links to when it ended at that one's head and the cell holds the link
//! (`luna_core::__private::SideCell`), else returned for the dispatcher.

use super::side_call::switch_sealed;
use super::*;
use luna_core::__private::{EXIT_RELINK, SideCell};

/// Branches on `r`, the return of the side trace cell `c` holds: to
/// `back` when it ends at this trace's head; into the linked trace when it
/// ends at that one's head and the link may be taken, whose return is
/// returned; to `other` otherwise.
pub(super) fn emit_branch_on_return<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    c: Value,
    r: Value,
    back: Block,
    other: Block,
) {
    let (at, flags) = (SideCell::offsets(), MemFlagsData::trusted());
    let relinked = lw.bcx.ins().band_imm_u(r, EXIT_RELINK as i64);
    let tagged = lw.bcx.create_block();
    lw.bcx.ins().brif(relinked, tagged, &[], other, &[]);
    switch_sealed(lw, tagged);
    let pc = lw.bcx.ins().ireduce(types::I32, r);
    let head = lw
        .bcx
        .ins()
        .icmp_imm_u(IntCC::Equal, pc, i64::from(pl.record.head_pc));
    let elsewhere = lw.bcx.create_block();
    lw.bcx.ins().brif(head, back, &[], elsewhere, &[]);
    switch_sealed(lw, elsewhere);
    let link_pc = lw.bcx.ins().load(types::I32, flags, c, at.link_pc);
    let linked = lw.bcx.ins().icmp(IntCC::Equal, pc, link_pc);
    let take = lw.bcx.create_block();
    lw.bcx.ins().brif(linked, take, &[], other, &[]);
    switch_sealed(lw, take);
    let link = lw.bcx.ins().load(types::I64, flags, c, at.link);
    let f = lw.bcx.import_func(lw.h.rt.side_cell_list_id);
    let call = lw.bcx.ins().call(f, &[link, lw.entry.reg_state]);
    let entry = lw.bcx.inst_results(call)[0];
    let enter = lw.bcx.create_block();
    lw.bcx.ins().brif(entry, enter, &[], other, &[]);
    switch_sealed(lw, enter);
    let sig = lw.entry.trace_fn_sig_ref;
    let call = lw
        .bcx
        .ins()
        .call_indirect(sig, entry, &[lw.entry.reg_state]);
    let ret = lw.bcx.inst_results(call)[0];
    lw.bcx.ins().return_(&[ret]);
}
