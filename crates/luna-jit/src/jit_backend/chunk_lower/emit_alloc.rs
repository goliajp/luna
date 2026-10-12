//! Code that allocates: `OP_NEWTABLE`, and leaving the compiled call when a
//! helper could not allocate.

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
    let EmitState {
        current_kinds,
        current_is_nil,
        ..
    } = st;
    match ins.op() {
        Op::NewTable => {
            // `R[A] = {...}` calls `luna_jit_new_table_sized` with the
            // NewTable's operands, which size the table as the interpreter
            // does; a counted `for i = 1, N` fill right after (see
            // `presize_hints`) passes the size it ends at instead
            let ops = presize_for_newtable
                .get(&pc)
                .copied()
                .unwrap_or_else(|| crate::jit_backend::pack_table_ops(ins));
            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let id = module
                .declare_function("luna_jit_new_table_sized", Linkage::Import, &sig)
                .ok()?;
            let r = module.declare_func_in_func(id, bcx.func);
            let ops_v = bcx.ins().iconst(types::I64, ops);
            let call_inst = bcx.ins().call(r, &[ops_v]);
            let g = bcx.inst_results(call_inst)[0];
            // 0: the table could not be made (the helper parked the error
            // or the deopt); the interpreter runs the call
            emit_bail_if_zero(module, bcx, g, false)?;
            aligned_def(bcx, regs, reg_kinds, ins.a() as usize, g);
            current_kinds[ins.a() as usize] = RegKind::Table;
            current_is_nil[ins.a() as usize] = false;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

/// Leave the compiled call when `v`, a helper's result, is 0: the helper
/// could not allocate (or store) and the interpreter runs the call. With
/// `park`, a deopt is parked here; otherwise the helper parked it.
pub(super) fn emit_bail_if_zero<M: Module>(
    module: &mut M,
    bcx: &mut FunctionBuilder<'_>,
    v: Value,
    park: bool,
) -> Option<()> {
    let ok_blk = bcx.create_block();
    let bail_blk = bcx.create_block();
    bcx.ins().brif(v, ok_blk, &[], bail_blk, &[]);
    bcx.switch_to_block(bail_blk);
    bcx.seal_block(bail_blk);
    if park {
        let park_id = module
            .declare_function(
                "luna_jit_park_deopt",
                Linkage::Import,
                &module.make_signature(),
            )
            .ok()?;
        let park_ref = module.declare_func_in_func(park_id, bcx.func);
        bcx.ins().call(park_ref, &[]);
    }
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);
    bcx.switch_to_block(ok_blk);
    bcx.seal_block(ok_blk);
    Some(())
}
