//! A level handing itself to the interpreter (`luna_jit_handoff_frame`):
//! before an op whose collection check would find a step due, or after a
//! self call whose callee handed itself over or did not start.

use super::*;
use luna_core::runtime::value::raw;
use luna_jit_helpers::{SELF_FAILED, SELF_HANDED, SELF_NOT_STARTED};

/// Record this level's frame, which the interpreter goes on with at `pc`
/// (a value), with its registers as `st` has them, and return; a level of
/// a self-recursive chunk sets the context's flag for the one below.
pub(super) fn emit_handoff<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &EmitState,
    f: EmitFacts<'_>,
    pc: Value,
) -> Option<()> {
    let n = f.c.proto.max_stack as usize;
    let slot = bcx.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        (16 * n.max(1)) as u32,
        3,
    ));
    for r in 0..n {
        let tag = match st.current_kinds.get(r) {
            _ if st.current_is_nil.get(r).copied().unwrap_or(false) => raw::NIL,
            Some(RegKind::Int) => raw::INT,
            Some(RegKind::Float) => raw::FLOAT,
            Some(RegKind::Table) => raw::TABLE,
            Some(RegKind::Unset) | None => raw::NIL,
        };
        let off = (16 * r) as i32;
        let t = bcx.ins().iconst(types::I64, i64::from(tag));
        bcx.ins().stack_store(types::I64, t, slot, off);
        if tag != raw::NIL {
            let v = bcx.use_var(f.regs[r]);
            bcx.ins().stack_store(types::I64, v, slot, off + 8);
        }
    }
    let mut sig = module.make_signature();
    for _ in 0..4 {
        sig.params.push(AbiParam::new(types::I64));
    }
    let id = module
        .declare_function("luna_jit_handoff_frame", Linkage::Import, &sig)
        .ok()?;
    let fref = module.declare_func_in_func(id, bcx.func);
    let base = match f.self_calls {
        Some(sc) => sc.base,
        None => bcx.ins().iconst(types::I64, -1),
    };
    let regs = bcx.ins().stack_addr(types::I64, slot, 0);
    let count = bcx.ins().iconst(types::I64, n as i64);
    bcx.ins().call(fref, &[pc, base, regs, count]);
    if f.self_calls.is_some() {
        let ctx = bcx.ins().get_pinned_reg(types::I64);
        let handed = bcx.ins().iconst(types::I64, SELF_HANDED);
        bcx.ins().store(MemFlagsData::trusted(), handed, ctx, 8);
    }
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);
    Some(())
}

/// After a self call at `pc`: with the context's flag set, return at once
/// (a call failed) or hand this level over, going on after the call (the
/// callee handed itself over) or with the call (it did not start).
pub(super) fn emit_after_self_call<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &EmitState,
    f: EmitFacts<'_>,
    pc: usize,
) -> Option<()> {
    let flags = MemFlagsData::trusted();
    let ctx = bcx.ins().get_pinned_reg(types::I64);
    let flag = bcx.ins().load(types::I64, flags, ctx, 8);
    let cold = bcx.create_block();
    let failed = bcx.create_block();
    let hand = bcx.create_block();
    let go_on = bcx.create_block();
    bcx.set_cold_block(cold);
    bcx.set_cold_block(failed);
    bcx.set_cold_block(hand);
    bcx.ins().brif(flag, cold, &[], go_on, &[]);
    bcx.switch_to_block(cold);
    let is_failed = bcx.ins().icmp_imm_s(IntCC::Equal, flag, SELF_FAILED);
    bcx.ins().brif(is_failed, failed, &[], hand, &[]);
    bcx.switch_to_block(failed);
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);
    bcx.switch_to_block(hand);
    let not_started = bcx.ins().icmp_imm_s(IntCC::Equal, flag, SELF_NOT_STARTED);
    let old = i64::from(f.c.old_pc[pc]);
    let at = bcx.ins().iconst(types::I64, old);
    let after = bcx.ins().iconst(types::I64, old + 1);
    let resume = bcx.ins().select(not_started, at, after);
    emit_handoff(module, bcx, st, f, resume)?;
    bcx.switch_to_block(go_on);
    Some(())
}
