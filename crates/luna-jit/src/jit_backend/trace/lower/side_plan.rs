//! The call of a side cell's side trace with the cell's plan compiled in
//! (`luna_core::__private::CellPlan`): what the call block does for any
//! cell (see `side_call`), but the checks before the side trace runs, the
//! stores when it goes on in this trace's loop and those before the trace
//! it links to is entered are made here, reading and writing the stack
//! through the Vm's run words, instead of by the Vm from the cell's lists.
//!
//! The plan of a cell is set once, when the cell is filled, so code made
//! from it holds for as long as the cell does. Each planned cell has a call
//! block of its own; the relink block is the one every call goes on at.

use super::relinks::{Relinks, Way};
use super::side_call::{cold_block, switch_sealed};
use super::*;
use luna_core::__private::{
    CellPlan, EXIT_RELINK, RunWords, STACK_PTR_OFFSET, SideCell, SlotStore, VALUE_PAYLOAD,
    VALUE_SIZE, slot_check, slot_store, trace_ran_side_slot,
};

/// The address of the Vm's run words, a relocation: the one the expansion
/// set, which each Vm writes its own into.
pub(super) fn run_words<E: Emit>(bcx: &mut E) -> Value {
    let known = (bcx.relocs_mut().iter()).find_map(|r| (r.0 == RelocKind::RunWords).then_some(r.1));
    bcx.reloc(RelocKind::RunWords, known.unwrap_or(0))
}

/// The calling exit of cell `n`, whose plan is `plan`, returning `ret`
/// with note `noted` for the dispatcher: on to a call block of its own.
pub(crate) fn emit_planned_exit<E: Emit>(
    bcx: &mut E,
    x: &Relinks,
    way: &mut Way,
    n: u32,
    (ret, tags_idx): (Value, u32),
    plan: &CellPlan,
) {
    let c = super::side_call::cell_at(bcx, n);
    let noted = bcx.ins().iconst(types::I64, i64::from(tags_idx) + 1);
    let call = cold_block(bcx);
    let params: Vec<Value> = (0..3)
        .map(|_| bcx.append_block_param(call, types::I64))
        .collect();
    let args = [c, ret, noted].map(cranelift_codegen::ir::BlockArg::Value);
    bcx.calling_exit(n, call, &args);
    switch_sealed(bcx, call);
    let (c, ret, noted) = (params[0], params[1], params[2]);
    let out = cold_block(bcx);
    let entry = emit_entry_counted(bcx, c, out);
    emit_blocked(bcx, out);
    emit_checks(bcx, &plan.checks, out);
    let call = bcx.ins().call_indirect(x.sig, entry, &[x.reg_state]);
    let r = bcx.inst_results(call)[0];
    let (back, other) = (cold_block(bcx), cold_block(bcx));
    emit_branch_on_return(bcx, x, plan, (c, r), back, other);
    switch_sealed(bcx, other);
    emit_noted_return(bcx, x, noted, r);
    switch_sealed(bcx, back);
    emit_back(bcx, x, way, plan, (c, noted));
    switch_sealed(bcx, out);
    bcx.ins().return_(&[ret]);
}

/// On to `blocked` when the exit left something for the dispatcher to do
/// (a deopt, a collection check, something to make).
pub(super) fn emit_blocked<E: Emit>(bcx: &mut E, blocked: Block) {
    let rw = run_words(bcx);
    let at = RunWords::offsets().blocked;
    let b = bcx.ins().load(types::I32, MemFlagsData::trusted(), rw, at);
    let free = cold_block(bcx);
    bcx.ins().brif(b, blocked, &[], free, &[]);
    switch_sealed(bcx, free);
}

/// The address of the running trace's frame's first value on the stack,
/// read now: the stack moves as it grows.
fn frame_values<E: Emit>(bcx: &mut E) -> Value {
    let (rw, flags, at) = (run_words(bcx), MemFlagsData::trusted(), RunWords::offsets());
    let stack = bcx.ins().load(types::I64, flags, rw, at.stack);
    let values = bcx.ins().load(types::I64, flags, stack, STACK_PTR_OFFSET);
    let base = bcx.ins().load(types::I32, flags, rw, at.base);
    let base = bcx.ins().uextend(types::I64, base);
    bcx.ins().iadd(values, base)
}

/// On to `fail` unless each slot of `checks` holds its raw tag on the
/// stack.
pub(super) fn emit_checks<E: Emit>(bcx: &mut E, checks: &[(u32, u8)], fail: Block) {
    if checks.is_empty() {
        return;
    }
    let (frame, flags) = (frame_values(bcx), MemFlagsData::trusted());
    for &(slot, raw) in checks {
        let off = slot as i32 * VALUE_SIZE;
        let (tag, truth) = slot_check(raw);
        let want = |bcx: &mut E, at: i32, byte: u8| {
            let b = bcx.ins().uload8(types::I32, flags, frame, at);
            let other = bcx.ins().icmp_imm_u(IntCC::NotEqual, b, i64::from(byte));
            let ok = cold_block(bcx);
            bcx.ins().brif(other, fail, &[], ok, &[]);
            switch_sealed(bcx, ok);
        };
        want(bcx, off, tag);
        if let Some(t) = truth {
            want(bcx, off + VALUE_PAYLOAD, t);
        }
    }
}

/// Each slot of `stores` written to the stack from its register in
/// `reg_state`, as the exit's write-back writes it.
pub(crate) fn emit_stores<E: Ins>(
    bcx: &mut E,
    reg_state: Value,
    frame: Value,
    stores: &[(u32, u8)],
) {
    let flags = MemFlagsData::trusted();
    for &(slot, d) in stores {
        let off = slot as i32 * VALUE_SIZE;
        let (tag, payload) = match slot_store(d) {
            SlotStore::Leave => continue,
            SlotStore::Tag(t) => (t, None),
            SlotStore::Bool(t) => (t, Some(true)),
            SlotStore::Word(t) => (t, Some(false)),
        };
        let t = bcx.ins().iconst(types::I8, i64::from(tag));
        bcx.ins().store(flags, t, frame, off);
        if let Some(bit) = payload {
            let r = bcx
                .ins()
                .load(types::I64, flags, reg_state, 8 * slot as i32);
            let r = if bit { bcx.ins().band_imm_u(r, 1) } else { r };
            bcx.ins().store(flags, r, frame, off + VALUE_PAYLOAD);
        }
    }
}

/// [`emit_stores`] into the running trace's frame, read now.
pub(super) fn emit_frame_stores<E: Emit>(bcx: &mut E, x: &Relinks, stores: &[(u32, u8)]) {
    if stores
        .iter()
        .all(|&(_, d)| slot_store(d) == SlotStore::Leave)
    {
        return;
    }
    let frame = frame_values(bcx);
    emit_stores(bcx, x.reg_state, frame, stores);
}

/// Branches on `r`, the return of the side trace of cell `c`: to `back`
/// when it ends at this trace's head; into the linked trace when it ends
/// at that one's head and the link may be taken, whose return is
/// returned; to `other` otherwise.
fn emit_branch_on_return<E: Emit>(
    bcx: &mut E,
    x: &Relinks,
    plan: &CellPlan,
    (c, r): (Value, Value),
    back: Block,
    other: Block,
) {
    let relinked = bcx.ins().band_imm_u(r, EXIT_RELINK as i64);
    let tagged = cold_block(bcx);
    bcx.ins().brif(relinked, tagged, &[], other, &[]);
    switch_sealed(bcx, tagged);
    let pc = bcx.ins().ireduce(types::I32, r);
    let head = bcx.ins().icmp_imm_u(IntCC::Equal, pc, i64::from(x.head_pc));
    let Some(link) = &plan.link else {
        bcx.ins().brif(head, back, &[], other, &[]);
        return;
    };
    let elsewhere = cold_block(bcx);
    bcx.ins().brif(head, back, &[], elsewhere, &[]);
    switch_sealed(bcx, elsewhere);
    let linked = bcx.ins().icmp_imm_u(IntCC::Equal, pc, i64::from(link.pc));
    let take = cold_block(bcx);
    bcx.ins().brif(linked, take, &[], other, &[]);
    switch_sealed(bcx, take);
    super::plan_link::emit_link(bcx, x, c, link, other);
}

/// Back into this trace's loop: the cell's relink count bumped, its stores
/// made, and on at the relink block with the word it reads.
fn emit_back<E: Emit>(
    bcx: &mut E,
    x: &Relinks,
    way: &mut Way,
    plan: &CellPlan,
    (c, noted): (Value, Value),
) {
    let (at, flags) = (SideCell::offsets(), MemFlagsData::trusted());
    let k = bcx.ins().load(types::I32, flags, c, at.relinks);
    let k = bcx.ins().iadd_imm_s(k, 1);
    bcx.ins().store(flags, k, c, at.relinks);
    emit_frame_stores(bcx, x, &plan.stores);
    let ok = bcx.ins().iconst(types::I64, i64::from(plan.loop_ok));
    emit_relink_word(bcx, x, way, ok, noted);
}

/// The side trace's entry in cell `c`, its move to the optimizing tier
/// counted: on to `out` when the cell is empty or the move is due (the
/// dispatcher makes it).
pub(super) fn emit_entry_counted<E: Emit>(bcx: &mut E, c: Value, out: Block) -> Value {
    let (at, flags) = (SideCell::offsets(), MemFlagsData::trusted());
    let entry = bcx.ins().load(types::I64, flags, c, at.entry);
    let count_blk = cold_block(bcx);
    bcx.ins().brif(entry, count_blk, &[], out, &[]);
    switch_sealed(bcx, count_blk);
    let cp = bcx.ins().load(types::I64, flags, c, at.count);
    let n0 = bcx.ins().load(types::I32, flags, cp, 0);
    let n1 = bcx.ins().iadd_imm_s(n0, 1);
    bcx.ins().store(flags, n1, cp, 0);
    let limit = bcx.ins().load(types::I32, flags, c, at.at);
    let reached = bcx.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, n1, limit);
    let go = cold_block(bcx);
    bcx.ins().brif(reached, out, &[], go, &[]);
    switch_sealed(bcx, go);
    entry
}

/// `r`, the side trace's return, returned for the dispatcher with `noted`
/// (the typed exit + 1) stored where it reads it.
pub(super) fn emit_noted_return<E: Emit>(bcx: &mut E, x: &Relinks, noted: Value, r: Value) {
    let slot = 8 * trace_ran_side_slot(x.window as usize) as i32;
    (bcx.ins()).store(MemFlagsData::trusted(), noted, x.reg_state, slot);
    bcx.ins().return_(&[r]);
}

/// On at the relink block, the word it reads left with `ok` (whether the
/// side trace may go on in the optimizing tier's loop) and `noted`.
pub(super) fn emit_relink_word<E: Emit>(
    bcx: &mut E,
    x: &Relinks,
    way: &mut Way,
    ok: Value,
    noted: Value,
) {
    let slot = 8 * trace_ran_side_slot(x.window as usize) as i32;
    // the relink block takes it from the word with the note, as it does
    // from the baseline tier's call stub (see `reentry::count_relink`)
    let noted = bcx.ins().ishl_imm_u(noted, 32);
    let word = bcx.ins().bor(ok, noted);
    (bcx.ins()).store(MemFlagsData::trusted(), word, x.reg_state, slot);
    let block = match way.relink {
        Some(b) => b,
        None => *way.relink.insert(cold_block(bcx)),
    };
    bcx.ins().jump(block, &[]);
}
