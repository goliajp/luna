//! Replaying a baseline trace's instructions into a Cranelift function.

use super::*;

/// The blocks the replay of `i` branches to (only the continuation of a
/// `TierCount`, which Cranelift code does not count).
fn targets(i: &Inst) -> [Option<u32>; 2] {
    match i.op {
        Op::Jump => [Some(i.a), None],
        Op::Brif(_) => [Some(i.b), Some(i.c)],
        Op::TierCount { .. } => [Some(i.c), None],
        _ => [None, None],
    }
}

/// Defines the trace function in `module` from `lir`.
/// `relocs`: the addresses the relocations stand for in the Vm the code is
/// for.
pub(crate) fn define<M: Module>(
    lir: &Lir,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
    module: &mut M,
) -> Option<FuncId> {
    let mut an = live::Analysis::default();
    live::analyze(lir, &mut an);
    let call_conv = module.isa().default_call_conv();
    let read_cost = ops::read_in_place_cost(module.isa().triple().architecture);
    let mut sig = Signature::new(call_conv);
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let fn_id = module
        .declare_function("luna_jit_trace", Linkage::Local, &sig)
        .ok()?;
    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, fn_id.as_u32());
    let mut fbc = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    let mut blocks = vec![None; lir.blocks.len()];
    for &blk in &an.order {
        blocks[blk as usize] = Some(b.create_block());
    }
    let mut vals = vec![None; lir.value_ty.len()];
    let entry = blocks[0].expect("the entry block");
    b.append_block_params_for_function_params(entry);
    if lir.arg0 != NONE {
        vals[lir.arg0 as usize] = Some(b.block_params(entry)[0]);
    }
    for &blk in &an.order {
        if blk == 0 {
            continue;
        }
        let cb = blocks[blk as usize].expect("laid out");
        for &p in lir.block_params(blk) {
            vals[p as usize] = Some(b.append_block_param(cb, cty(lir.value_ty[p as usize])));
        }
    }
    let vars = lir.var_ty.iter().map(|&t| b.declare_var(cty(t))).collect();
    let slots = lir
        .slots
        .iter()
        .map(|&(size, align)| {
            b.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                size,
                align,
            ))
        })
        .collect();
    let mut reloc_gv = Vec::with_capacity(relocs.len());
    for n in 0..relocs.len() {
        let id = module
            .declare_data(
                &crate::jit_backend::trace::reloc_symbol(n),
                Linkage::Import,
                false,
                false,
            )
            .ok()?;
        reloc_gv.push(module.declare_data_in_func(id, b.func));
    }
    let mut r = Replay {
        lir,
        b,
        vals,
        vars,
        blocks,
        slots,
        call_conv,
        hoisted: vec![None; relocs.len()],
        reloc_gv,
    };
    // each block is sealed once its last predecessor branches to it: with
    // every block left open until the end, Cranelift's SSA construction
    // gives loop heads and merges a parameter for each variable read there,
    // and the loop carries all of them
    let mut preds = vec![0u32; lir.blocks.len()];
    let mut sealed = vec![false; lir.blocks.len()];
    for &blk in &an.order {
        let (lo, hi) = an.block_at[blk as usize];
        for c in lo..hi {
            for t in targets(&lir.insts[an.code[c as usize] as usize])
                .into_iter()
                .flatten()
            {
                preds[t as usize] += 1;
            }
        }
    }
    for &blk in &an.order {
        let cb = r.blocks[blk as usize].expect("laid out");
        r.b.switch_to_block(cb);
        if blk == 0 {
            hoist(&mut r, &an, relocs, read_cost);
        }
        // a block no branch reaches (the entry, or the hot exit of a
        // `TierCount`, which the replay does not take) is sealed as it starts
        if preds[blk as usize] == 0 && !sealed[blk as usize] {
            r.b.seal_block(cb);
            sealed[blk as usize] = true;
        }
        let (lo, hi) = an.block_at[blk as usize];
        for c in lo..hi {
            let i = lir.insts[an.code[c as usize] as usize];
            r.inst(&i);
            for t in targets(&i).into_iter().flatten() {
                preds[t as usize] -= 1;
                if preds[t as usize] == 0 {
                    r.b.seal_block(r.blocks[t as usize].expect("laid out"));
                    sealed[t as usize] = true;
                }
            }
        }
    }
    r.b.finalize(module.target_config());
    crate::jit_backend::trace::drop_unused_block_params(&mut ctx.func);
    // `LUNA_TRACE_IR_DUMP=1`, as for the lowerer's own Cranelift IR
    if std::env::var_os("LUNA_TRACE_IR_DUMP").is_some_and(|v| v == "1") {
        eprintln!(
            "=== TRACE IR DUMP (from baseline) ===\n{}\n=== END ===",
            ctx.func.display()
        );
    }
    // and `LUNA_TRACE_ASM_DUMP=1` for the machine code
    let asm_dump = std::env::var_os("LUNA_TRACE_ASM_DUMP").is_some_and(|v| v == "1");
    if asm_dump {
        ctx.set_disasm(true);
    }
    crate::jit_backend::trace::reloc::set_values(relocs);
    crate::jit_backend::trace::reloc::define_aligned(&mut *module, fn_id, &mut ctx)?;
    if asm_dump && let Some(vcode) = ctx.compiled_code().and_then(|c| c.vcode.as_ref()) {
        eprintln!("=== TRACE ASM DUMP (from baseline) ===\n{vcode}\n=== END ===");
    }
    module.clear_context(&mut ctx);
    Some(fn_id)
}
