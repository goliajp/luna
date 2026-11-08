//! The return of a side trace a trace's code ran from a side cell: on in
//! the trace's own loop when it ended at the trace's head, into the trace
//! it links to when it ended at that one's head and the cell holds the link
//! (`luna_core::__private::SideCell`), else returned for the dispatcher.

use super::relinks::Relinks;
use super::side_call::{cold_block, switch_sealed};
use super::*;
use luna_core::__private::{EXIT_RELINK, SideCell};

/// Branches on `r`, the return of the side trace cell `c` holds: to
/// `back` when it ends at this trace's head; into the linked trace when it
/// ends at that one's head and the link may be taken, whose return is
/// returned; to `other` otherwise.
pub(super) fn emit_branch_on_return<E: Emit>(
    bcx: &mut E,
    x: &Relinks,
    c: Value,
    r: Value,
    back: Block,
    other: Block,
) {
    let (at, flags) = (SideCell::offsets(), MemFlagsData::trusted());
    let relinked = bcx.ins().band_imm_u(r, EXIT_RELINK as i64);
    let tagged = cold_block(bcx);
    bcx.ins().brif(relinked, tagged, &[], other, &[]);
    switch_sealed(bcx, tagged);
    let pc = bcx.ins().ireduce(types::I32, r);
    let head = bcx.ins().icmp_imm_u(IntCC::Equal, pc, i64::from(x.head_pc));
    let elsewhere = cold_block(bcx);
    bcx.ins().brif(head, back, &[], elsewhere, &[]);
    switch_sealed(bcx, elsewhere);
    let link_pc = bcx.ins().load(types::I32, flags, c, at.link_pc);
    let linked = bcx.ins().icmp(IntCC::Equal, pc, link_pc);
    let take = cold_block(bcx);
    bcx.ins().brif(linked, take, &[], other, &[]);
    switch_sealed(bcx, take);
    let link = bcx.ins().load(types::I64, flags, c, at.link);
    let call = bcx.ins().call(x.list_fn, &[link, x.reg_state]);
    let entry = bcx.inst_results(call)[0];
    let enter = cold_block(bcx);
    bcx.ins().brif(entry, enter, &[], other, &[]);
    switch_sealed(bcx, enter);
    let call = bcx.ins().call_indirect(x.sig, entry, &[x.reg_state]);
    let ret = bcx.inst_results(call)[0];
    bcx.ins().return_(&[ret]);
}
