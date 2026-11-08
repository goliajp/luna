//! Entering the trace a side trace links to (see `side_plan`), done by the
//! code itself as the Vm does it (`relink::enter`'s `link_take`): the
//! link noted on the trail, the stores, the words of the two traces' calls,
//! the running trace and the entry. What the code cannot see to, it leaves
//! to the Vm, which does all of it from the cell's list: the exit left
//! something to the dispatcher, hot counting pauses, the links in a row are
//! at their limit or the trail has no room, the linked trace has yet to
//! move to the optimizing tier or inlines calls (whose room only the Vm
//! knows).

use super::relinks::Relinks;
use super::side_call::{cold_block, switch_sealed};
use super::side_plan::{emit_blocked, emit_checks, emit_frame_stores, run_words};
use super::*;
use luna_core::__private::{
    LINK_DEPTH, LinkPlan, RunWords, SideCell, trace_count_slot, trace_passed_slot,
    trace_ran_side_slot,
};

/// Into the trace `link` names, cell `c`'s side trace having returned at
/// its head: its return returned; to `other` when a check before it fails
/// or the Vm does not take it.
pub(super) fn emit_link<E: Emit>(
    bcx: &mut E,
    x: &Relinks,
    c: Value,
    link: &LinkPlan,
    other: Block,
) {
    let slow = cold_block(bcx);
    if link.target_inline_depth == 0 {
        emit_fast(bcx, x, c, link, (slow, other));
    } else {
        bcx.ins().jump(slow, &[]);
    }
    switch_sealed(bcx, slow);
    let flags = MemFlagsData::trusted();
    let (rw, one) = (run_words(bcx), bcx.ins().iconst(types::I32, 1));
    bcx.ins()
        .store(flags, one, rw, RunWords::offsets().link_left);
    let list = bcx
        .ins()
        .load(types::I64, flags, c, SideCell::offsets().link);
    let call = bcx.ins().call(x.list_fn, &[list, x.reg_state]);
    let entry = bcx.inst_results(call)[0];
    emit_enter(bcx, x, entry, other);
}

/// The call of the entry `entry` returned, or on to `other` when it is 0.
fn emit_enter<E: Emit>(bcx: &mut E, x: &Relinks, entry: Value, other: Block) {
    let enter = cold_block(bcx);
    bcx.ins().brif(entry, enter, &[], other, &[]);
    switch_sealed(bcx, enter);
    let call = bcx.ins().call_indirect(x.sig, entry, &[x.reg_state]);
    let ret = bcx.inst_results(call)[0];
    bcx.ins().return_(&[ret]);
}

/// The link taken by the code, or on to `slow` (see the module comment).
fn emit_fast<E: Emit>(
    bcx: &mut E,
    x: &Relinks,
    c: Value,
    link: &LinkPlan,
    (slow, other): (Block, Block),
) {
    let (flags, at, rw) = (MemFlagsData::trusted(), RunWords::offsets(), run_words(bcx));
    emit_blocked(bcx, slow);
    let quiet = bcx.ins().load(types::I32, flags, rw, at.quiet);
    branch_on(bcx, quiet, slow);
    let len = bcx.ins().load(types::I32, flags, rw, at.trail_len);
    let base = bcx.ins().load(types::I32, flags, rw, at.link_base);
    let depth = bcx.ins().isub(len, base);
    let deep = bcx
        .ins()
        .icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, depth, LINK_DEPTH as i64);
    branch_on(bcx, deep, slow);
    let cap = bcx.ins().load(types::I32, flags, rw, at.trail_cap);
    let full = bcx.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, len, cap);
    branch_on(bcx, full, slow);
    emit_checks(bcx, &link.checks, other);
    let lc = SideCell::offsets();
    if link.target_tiers {
        let tried = bcx.ins().load(types::I64, flags, c, lc.link_tried);
        let tried = bcx.ins().uload8(types::I32, flags, tried, 0);
        let moving = bcx.ins().icmp_imm_u(IntCC::Equal, tried, 0);
        branch_on(bcx, moving, slow);
    }
    let cell = bcx.ins().load(types::I64, flags, c, lc.link_entry);
    let entry = bcx.ins().load(types::I64, flags, cell, 0);
    let none = bcx.ins().icmp_imm_u(IntCC::Equal, entry, 0);
    branch_on(bcx, none, slow);
    emit_trail(bcx, rw, len, link.trail);
    emit_frame_stores(bcx, x, &link.stores);
    emit_call_words(bcx, x, c, link);
    let target = bcx.ins().load(types::I64, flags, c, lc.link_target);
    bcx.ins().store(flags, target, rw, at.running);
    let call = bcx.ins().call_indirect(x.sig, entry, &[x.reg_state]);
    let ret = bcx.inst_results(call)[0];
    bcx.ins().return_(&[ret]);
}

/// On to `to` when `cond` is non-zero, else on in a new block.
fn branch_on<E: Emit>(bcx: &mut E, cond: Value, to: Block) {
    let on = cold_block(bcx);
    bcx.ins().brif(cond, to, &[], on, &[]);
    switch_sealed(bcx, on);
}

/// `link` noted on the trail of the run words `rw`, which holds `len`
/// links and has room for another.
fn emit_trail<E: Emit>(bcx: &mut E, rw: Value, len: Value, link: [u32; 3]) {
    let (flags, at) = (MemFlagsData::trusted(), RunWords::offsets());
    let trail = bcx.ins().load(types::I64, flags, rw, at.trail_at);
    let n = bcx.ins().uextend(types::I64, len);
    let size = bcx.ins().iconst(types::I64, 12);
    let off = bcx.ins().imul(n, size);
    let slot = bcx.ins().iadd(trail, off);
    for (k, w) in link.into_iter().enumerate() {
        let w = bcx.ins().iconst(types::I32, i64::from(w));
        bcx.ins().store(flags, w, slot, 4 * k as i32);
    }
    let len = bcx.ins().iadd_imm_s(len, 1);
    bcx.ins().store(flags, len, rw, at.trail_len);
}

/// The words past the two traces' windows as the dispatcher leaves them
/// between the running trace and the linked one: the running trace's count
/// of this call back in its move, the registers past the frame cleared,
/// the linked trace's count and side-trace note set for its call.
fn emit_call_words<E: Emit>(bcx: &mut E, x: &Relinks, c: Value, link: &LinkPlan) {
    let (flags, at) = (MemFlagsData::trusted(), SideCell::offsets().link_counts);
    let regs = x.reg_state;
    let [prev, window, max_stack] = link.windows.map(|w| w as usize);
    let word = |s: usize| 8 * s as i32;
    if link.counts[0] {
        let w = bcx
            .ins()
            .load(types::I64, flags, regs, word(trace_count_slot(prev)));
        let w = bcx.ins().ireduce(types::I32, w);
        let count = bcx.ins().load(types::I64, flags, c, at);
        bcx.ins().store(flags, w, count, 0);
    }
    if window > max_stack {
        let z = bcx.ins().iconst(types::I64, 0);
        for r in max_stack..window {
            bcx.ins().store(flags, z, regs, word(r));
        }
    }
    let z = bcx.ins().iconst(types::I64, 0);
    if link.counts[1] {
        let count = bcx.ins().load(types::I64, flags, c, at + 8);
        let n = bcx.ins().load(types::I32, flags, count, 0);
        let n = bcx.ins().uextend(types::I64, n);
        let hi = bcx.ins().ishl_imm_u(n, 32);
        let w = bcx.ins().bor(hi, n);
        bcx.ins()
            .store(flags, w, regs, word(trace_count_slot(window)));
        bcx.ins()
            .store(flags, z, regs, word(trace_passed_slot(window)));
    }
    if link.target_cells {
        bcx.ins()
            .store(flags, z, regs, word(trace_ran_side_slot(window)));
    }
}
