//! Code that allocates: `OP_NEWTABLE`, and handing the level to the
//! interpreter when a helper could not run an op.

use super::*;

pub(super) fn emit_new_table<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        reg_kinds,
        presize_for_newtable,
        regs,
        ..
    } = f;
    match ins.op() {
        Op::NewTable => {
            // `R[A] = {...}` calls `luna_jit_new_table_sized` with the
            // NewTable's operands, which size the table as the interpreter
            // does; a counted `for i = 1, N` fill right after (see
            // `presize_hints`) passes the size it ends at instead
            let ops = crate::jit_backend::pack_table_ops(ins);
            let presized = presize_for_newtable.get(&pc).copied().unwrap_or(ops);
            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = module
                .declare_function("luna_jit_chunk_new_table", Linkage::Import, &sig)
                .ok()?;
            let r = module.declare_func_in_func(id, bcx.func);
            let ops_v = bcx.ins().iconst(types::I64, ops);
            let presized_v = bcx.ins().iconst(types::I64, presized);
            let call_inst = bcx.ins().call(r, &[ops_v, presized_v]);
            let g = bcx.inst_results(call_inst)[0];
            // 0: the interpreter runs this `NewTable`, from this level on
            emit_hand_if_zero(module, bcx, st, f, pc, g)?;
            aligned_def(bcx, regs, reg_kinds, ins.a() as usize, g);
            st.current_kinds[ins.a() as usize] = RegKind::Table;
            st.current_is_nil[ins.a() as usize] = false;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

/// Hand this level to the interpreter at the op at `pc` when `v`, a
/// helper's result, is 0: the helper could not run the op (a metatable, a
/// value of another kind, memory), and the interpreter runs it.
pub(super) fn emit_hand_if_zero<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    st: &EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    v: Value,
) -> Option<()> {
    let ok_blk = bcx.create_block();
    let hand_blk = bcx.create_block();
    bcx.set_cold_block(hand_blk);
    bcx.ins().brif(v, ok_blk, &[], hand_blk, &[]);
    bcx.switch_to_block(hand_blk);
    bcx.seal_block(hand_blk);
    let at = bcx.ins().iconst(types::I64, i64::from(f.c.old_pc[pc]));
    super::handoff::emit_handoff(module, bcx, st, f, at)?;
    bcx.switch_to_block(ok_blk);
    bcx.seal_block(ok_blk);
    Some(())
}
