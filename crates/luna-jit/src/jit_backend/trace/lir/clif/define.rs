//! Replaying a baseline trace's instructions into a Cranelift function.

use super::*;

/// The blocks the replay of `i` branches to (of a `TierCount`, which
/// Cranelift code does not count, the continuation only, or the hot block
/// only in `Start::FirstPass`; of a `Pick`, its `a` in `Start::Loop`; of a
/// calling exit `calls` has not, none).
fn targets(i: &Inst, start: Start, calls: &Calls) -> [Option<u32>; 2] {
    match i.op {
        Op::Jump => [Some(i.a), None],
        Op::CallingExit(n) if calls.has(n) => [Some(i.a), None],
        Op::Pick if start == Start::Loop => [Some(i.a), None],
        Op::Pick => [Some(i.b), None],
        Op::Brif(_) => [Some(i.b), Some(i.c)],
        Op::TierCount { .. } if start == Start::FirstPass => [Some(i.b), None],
        Op::TierCount { .. } => [Some(i.c), None],
        _ => [None, None],
    }
}

/// The loop's function's entry: the loop entry, which of its registers it
/// reads, and the values it takes from the back edges.
type EntryReads<'e> = (&'e LoopEntry, (Vec<bool>, Vec<(u32, u32)>));

/// Defines the function `start` names in `module` from `lir`: the trace,
/// or the optimizing tier's first pass or loop of a trace whose back edge
/// calls the loop (see `lower::handoff`; `None` when `lir` has no loop
/// entry, or one the replay cannot start from).
/// `relocs`: the addresses the relocations stand for in the Vm the code is
/// for; `calls`: the calling exits that go on to the call block.
pub(crate) fn define<M: Module>(
    lir: &Lir,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
    module: &mut M,
    start: Start,
    calls: &Calls,
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
    live::analyze_from(lir, &mut an, start, calls);
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
    let reloc_gv = reloc_globals(module, &mut b, relocs.len())?;
    let mut r = Replay::open(lir, b, &an, root, call_conv, start, reloc_gv);
    if let Some((_, (_, taken))) = &reads {
        r.inherit_vars(taken, &an);
    }
    replay_blocks(&mut r, &an, root, reads.as_ref(), relocs, read_cost);
    r.b.finalize(module.target_config());
    compile(module, fn_id, ctx, relocs)
}

/// The data symbols the relocations are read from, declared in `b`'s
/// function.
fn reloc_globals<M: Module>(
    module: &mut M,
    b: &mut FunctionBuilder<'_>,
    n: usize,
) -> Option<Vec<cranelift_codegen::ir::GlobalValue>> {
    let mut reloc_gv = Vec::with_capacity(n);
    for k in 0..n {
        let name = crate::jit_backend::trace::reloc_symbol(k);
        let id = module
            .declare_data(&name, Linkage::Import, false, false)
            .ok()?;
        reloc_gv.push(module.declare_data_in_func(id, b.func));
    }
    Some(reloc_gv)
}

impl<'a, 'f> Replay<'a, 'f> {
    /// A replay of `lir` into `b`, with a block for each block `an` lays
    /// out (`root` taking the function's parameter), a variable for each
    /// LIR variable and a stack slot for each LIR slot.
    fn open(
        lir: &'a Lir,
        mut b: FunctionBuilder<'f>,
        an: &live::Analysis,
        root: u32,
        call_conv: CallConv,
        start: Start,
        reloc_gv: Vec<cranelift_codegen::ir::GlobalValue>,
    ) -> Self {
        let mut blocks = vec![None; lir.blocks.len()];
        for &blk in &an.order {
            let cb = b.create_block();
            if lir.blocks[blk as usize].cold && blk != root {
                b.set_cold_block(cb);
            }
            blocks[blk as usize] = Some(cb);
        }
        let mut vals = vec![None; lir.value_ty.len()];
        let entry = blocks[root as usize].expect("the entry block");
        b.append_block_params_for_function_params(entry);
        if lir.arg0 != NONE {
            vals[lir.arg0 as usize] = Some(b.block_params(entry)[0]);
        }
        for &blk in an.order.iter().filter(|&&blk| blk != root) {
            let cb = blocks[blk as usize].expect("laid out");
            for &p in lir.block_params(blk) {
                vals[p as usize] = Some(b.append_block_param(cb, cty(lir.value_ty[p as usize])));
            }
        }
        let vars = lir.var_ty.iter().map(|&t| b.declare_var(cty(t))).collect();
        let slots = stack_slots(lir, &mut b);
        Replay {
            lir,
            b,
            vals,
            vars,
            blocks,
            slots,
            call_conv,
            hoisted: vec![None; reloc_gv.len()],
            reloc_gv,
            start,
            calls: an.exits.clone(),
            ivars: Vec::new(),
            defs: vec![None; lir.value_ty.len()],
            idom: Vec::new(),
        }
    }
}

fn stack_slots(lir: &Lir, b: &mut FunctionBuilder<'_>) -> Vec<cranelift_codegen::ir::StackSlot> {
    lir.slots
        .iter()
        .map(|&(size, align)| {
            b.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                size,
                align,
            ))
        })
        .collect()
}

/// Per block, how many branches the replay takes to it.
fn count_preds(lir: &Lir, an: &live::Analysis) -> Vec<u32> {
    let mut preds = vec![0u32; lir.blocks.len()];
    for &blk in &an.order {
        let (lo, hi) = an.block_at[blk as usize];
        for c in lo..hi {
            for t in targets(&lir.insts[an.code[c as usize] as usize], an.func, &an.exits)
                .into_iter()
                .flatten()
            {
                preds[t as usize] += 1;
            }
        }
    }
    preds
}

/// Replays the blocks in `an`'s order, reading the relocations and (in the
/// loop's function) the registers in `root`.
fn replay_blocks(
    r: &mut Replay<'_, '_>,
    an: &live::Analysis,
    root: u32,
    reads: Option<&EntryReads<'_>>,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
    read_cost: u32,
) {
    let lir = r.lir;
    // each block is sealed once its last predecessor branches to it: with
    // every block left open until the end, Cranelift's SSA construction
    // gives loop heads and merges a parameter for each variable read there,
    // and the loop carries all of them
    let mut preds = count_preds(lir, an);
    let mut sealed = vec![false; lir.blocks.len()];
    for &blk in &an.order {
        let cb = r.blocks[blk as usize].expect("laid out");
        r.b.switch_to_block(cb);
        if blk == root {
            hoist(r, an, relocs, read_cost);
            if let Some((e, (reads, _))) = reads {
                r.read_regs(&e.regs, reads);
                r.own_reg_state();
            }
        } else if lir.blocks[blk as usize].reentry {
            ops::rehoist(r);
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
            branched(r, &i, &mut preds, &mut sealed);
        }
    }
}

/// Counts the branches `i` takes, sealing each block whose last
/// predecessor this is.
fn branched(r: &mut Replay<'_, '_>, i: &Inst, preds: &mut [u32], sealed: &mut [bool]) {
    for t in targets(i, r.start, &r.calls).into_iter().flatten() {
        preds[t as usize] -= 1;
        if preds[t as usize] == 0 {
            r.b.seal_block(r.blocks[t as usize].expect("laid out"));
            sealed[t as usize] = true;
        }
    }
}

/// Compiles the finished function into `module` with `relocs` as the
/// relocations' values.
fn compile<M: Module>(
    module: &mut M,
    fn_id: FuncId,
    mut ctx: cranelift_codegen::Context,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
) -> Option<FuncId> {
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
