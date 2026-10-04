//! Dropping the block parameters of a lowered trace that nothing reads.

use super::*;

/// Removes block parameters nothing reads, with the arguments branches
/// pass to them.
///
/// `sync_reg_state` and the exits compare every register's current value
/// with what reg_state holds, and asking the SSA builder for a register's
/// value at a loop head gives it a parameter there. A register the loop
/// only writes then travels around the loop as a parameter that no
/// instruction reads, live across every helper call: token_bucket's trace
/// carried six of those, which register allocation had to keep and spill.
/// Cranelift does not remove them, so this does: a parameter is used when
/// an instruction other than a branch reads it, or when it is passed on to
/// a parameter that is used.
pub(super) fn drop_unused_block_params(func: &mut cranelift_codegen::ir::Function) {
    use cranelift_codegen::ir::{BlockArg, ValueDef};
    use std::collections::HashSet;

    let dfg = &func.dfg;
    let mut used: HashSet<Value> = HashSet::new();
    // (target parameter, argument) for every branch edge
    let mut edges: Vec<(Value, Value)> = Vec::new();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            for &v in dfg.inst_args(inst) {
                used.insert(dfg.resolve_aliases(v));
            }
            for dest in dfg.insts[inst].branch_destination(&dfg.jump_tables, &dfg.exception_tables)
            {
                let params = dfg.block_params(dest.block(&dfg.value_lists));
                for (i, arg) in dest.args(&dfg.value_lists).enumerate() {
                    match arg {
                        BlockArg::Value(a) => edges.push((params[i], dfg.resolve_aliases(a))),
                        // a call result or exception payload: keep the parameter
                        _ => {
                            used.insert(params[i]);
                        }
                    }
                }
            }
        }
    }
    loop {
        let before = used.len();
        for &(param, arg) in &edges {
            if used.contains(&param) {
                used.insert(arg);
            }
        }
        if used.len() == before {
            break;
        }
    }
    let entry = func.layout.entry_block();
    let blocks: Vec<_> = func.layout.blocks().collect();
    for &block in &blocks {
        if Some(block) == entry {
            continue;
        }
        let dead: Vec<usize> = func
            .dfg
            .block_params(block)
            .iter()
            .enumerate()
            .filter(|(_, p)| !used.contains(p))
            .map(|(i, _)| i)
            .collect();
        if dead.is_empty() {
            continue;
        }
        for &src in &blocks {
            let insts: Vec<_> = func.layout.block_insts(src).collect();
            for inst in insts {
                let dfg = &mut func.dfg;
                for dest in dfg.insts[inst]
                    .branch_destination_mut(&mut dfg.jump_tables, &mut dfg.exception_tables)
                {
                    if dest.block(&dfg.value_lists) == block {
                        for &i in dead.iter().rev() {
                            dest.remove(i, &mut dfg.value_lists);
                        }
                    }
                }
            }
        }
        for &i in dead.iter().rev() {
            let p = func.dfg.block_params(block)[i];
            debug_assert!(matches!(func.dfg.value_def(p), ValueDef::Param(..)));
            func.dfg.remove_block_param(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cranelift_codegen::ir::{Function, Signature, UserFuncName};
    use cranelift_codegen::isa::{CallConv, TargetFrontendConfig};

    /// `loop(dead, live)`: `dead` is only passed back unchanged, as the
    /// registers a trace loop only writes were; `live` is read. The first
    /// parameter and its arguments go; the second stays.
    #[test]
    fn a_parameter_only_passed_back_to_itself_is_removed() {
        let mut sig = Signature::new(CallConv::SystemV);
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        let mut fctx = FunctionBuilderContext::new();
        let mut b = FunctionBuilder::new(&mut func, &mut fctx);
        let entry = b.create_block();
        let head = b.create_block();
        let out = b.create_block();
        b.append_block_params_for_function_params(entry);
        let dead = b.append_block_param(head, types::I64);
        let live = b.append_block_param(head, types::I64);
        b.switch_to_block(entry);
        let x = b.block_params(entry)[0];
        b.ins().jump(head, &[x.into(), x.into()]);
        b.switch_to_block(head);
        let one = b.ins().iconst(types::I64, 1);
        let again = b.ins().icmp_imm_s(IntCC::SignedLessThan, live, 10);
        let next_live = b.ins().iadd(live, one);
        b.ins()
            .brif(again, head, &[dead.into(), next_live.into()], out, &[]);
        b.switch_to_block(out);
        b.ins().return_(&[live]);
        b.seal_all_blocks();
        b.finalize(TargetFrontendConfig {
            default_call_conv: CallConv::SystemV,
            pointer_width: target_lexicon::PointerWidth::U64,
            page_size_align_log2: 12,
        });

        drop_unused_block_params(&mut func);

        assert_eq!(func.dfg.block_params(head), &[live]);
        let branches: Vec<usize> = func
            .layout
            .blocks()
            .flat_map(|blk| func.layout.block_insts(blk).collect::<Vec<_>>())
            .flat_map(|inst| {
                func.dfg.insts[inst]
                    .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables)
                    .iter()
                    .filter(|d| d.block(&func.dfg.value_lists) == head)
                    .map(|d| d.len(&func.dfg.value_lists))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(branches, vec![1, 1]);
        cranelift_codegen::verify_function(&func, &settings::Flags::new(settings::builder()))
            .expect("the function still verifies");
    }
}
