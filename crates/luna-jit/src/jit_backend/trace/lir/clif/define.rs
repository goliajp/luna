//! Replaying a baseline trace's instructions into a Cranelift function.

use super::*;

/// The blocks the replay of `i` branches to (of a `TierCount`, which
/// Cranelift code does not count, the continuation only, or the hot block
/// only in `Start::FirstPass`).
fn targets(i: &Inst, start: Start) -> [Option<u32>; 2] {
    match i.op {
        Op::Jump => [Some(i.a), None],
        Op::Brif(_) => [Some(i.b), Some(i.c)],
        Op::TierCount { .. } if start == Start::FirstPass => [Some(i.b), None],
        Op::TierCount { .. } => [Some(i.c), None],
        _ => [None, None],
    }
}

/// Defines the function `start` names in `module` from `lir`: the trace,
/// or the optimizing tier's first pass or loop of a trace whose back edge
/// calls the loop (see `lower::handoff`; `None` when `lir` has no loop
/// entry, or one the replay cannot start from).
/// `relocs`: the addresses the relocations stand for in the Vm the code is
/// for.
pub(crate) fn define<M: Module>(
    lir: &Lir,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
    module: &mut M,
    start: Start,
) -> Option<FuncId> {
    let entry_at = match (&lir.loop_entry, start) {
        (_, Start::Whole) => None,
        (Some(e), _) => Some(e),
        (None, _) => return None,
    };
    let root = match (entry_at, start) {
        (Some(e), Start::Loop) => e.block,
        _ => 0,
    };
    let mut an = live::Analysis::default();
    live::analyze_from(lir, &mut an, start);
    let reads = match (entry_at, start) {
        (Some(e), Start::Loop) => Some((e, entry::live_in_vars(lir, &an, e)?)),
        _ => None,
    };
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
    let entry = blocks[root as usize].expect("the entry block");
    b.append_block_params_for_function_params(entry);
    if lir.arg0 != NONE {
        vals[lir.arg0 as usize] = Some(b.block_params(entry)[0]);
    }
    for &blk in &an.order {
        if blk == root {
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
        start,
        ivars: Vec::new(),
        defs: vec![None; lir.value_ty.len()],
        idom: Vec::new(),
    };
    if let Some((_, (_, taken))) = &reads {
        r.inherit_vars(taken, &an);
    }
    // each block is sealed once its last predecessor branches to it: with
    // every block left open until the end, Cranelift's SSA construction
    // gives loop heads and merges a parameter for each variable read there,
    // and the loop carries all of them
    let mut preds = vec![0u32; lir.blocks.len()];
    let mut sealed = vec![false; lir.blocks.len()];
    for &blk in &an.order {
        let (lo, hi) = an.block_at[blk as usize];
        for c in lo..hi {
            for t in targets(&lir.insts[an.code[c as usize] as usize], start)
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
        if blk == root {
            hoist(&mut r, &an, relocs, read_cost);
            if let Some((e, (reads, _))) = &reads {
                r.read_regs(&e.regs, reads);
                r.own_reg_state();
            }
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
            if reads.is_some() {
                r.inherited_uses(&i, blk);
            }
            r.inst(&i);
            if reads.is_some() {
                r.inherited_def(&i, blk);
            }
            for t in targets(&i, start).into_iter().flatten() {
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
