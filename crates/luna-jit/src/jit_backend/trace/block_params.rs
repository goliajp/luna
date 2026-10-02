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
