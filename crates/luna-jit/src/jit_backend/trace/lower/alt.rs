use super::*;
use cranelift_codegen::ir::MemFlagsData;

/// In the side-exit block of comparison `i`: take its other way when it
/// has one the registers' kinds allow, jumping to `continue_blk` (the
/// recorded way's next op) or the rejoin block; `false` to leave instead.
pub(super) fn alt_taken<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    i: usize,
    continue_blk: Block,
) -> bool {
    let Plan {
        record,
        head_proto,
        max_stack,
        ..
    } = *pl;
    if !lw.call_chain.is_empty() {
        return false;
    }
    let kinds = &lw.current_kinds[..max_stack];
    match pl.alt_paths.get(i).cloned().flatten() {
        Some(alt_path::AltPath::Run(ops)) if alt_path::keeps_kinds(&ops, &head_proto, kinds) => {
            for inst in ops {
                let a = inst.a() as usize;
                let v = match inst.op() {
                    Op::Move => lw.bcx.use_var(lw.regs_full[inst.b() as usize]),
                    Op::LoadI => lw.bcx.ins().iconst(types::I64, inst.sbx() as i64),
                    Op::LoadF => {
                        let f = lw.bcx.ins().f64const(inst.sbx() as f64);
                        lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), f)
                    }
                    _ => match head_proto.consts[inst.bx() as usize] {
                        luna_core::runtime::Value::Int(n) => lw.bcx.ins().iconst(types::I64, n),
                        luna_core::runtime::Value::Float(f) => {
                            let f = lw.bcx.ins().f64const(f);
                            lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), f)
                        }
                        _ => unreachable!("simple loads are numbers"),
                    },
                };
                lw.bcx.def_var(lw.regs_full[a], v);
                // a value of either way past the rejoin
                lw.known_int[a] = None;
            }
            lw.bcx.ins().jump(continue_blk, &[]);
            true
        }
        Some(alt_path::AltPath::Skip { join, writes })
            if alt_path::keeps_kinds(
                &record.ops[i + 1..join]
                    .iter()
                    .map(|r| r.inst)
                    .collect::<Vec<_>>(),
                &head_proto,
                kinds,
            ) =>
        {
            let bcx = &mut lw.bcx;
            let blk = lw
                .alt_joins
                .entry(join)
                .or_insert_with(|| (bcx.create_block(), Vec::new()));
            blk.1.extend(writes);
            let blk = blk.0;
            lw.bcx.ins().jump(blk, &[]);
            true
        }
        _ => false,
    }
}

/// Rejoin, at recorded op `i`, the other ways that skipped to it.
pub(super) fn alt_join<M: Module>(lw: &mut Lower<'_, '_, M>, i: usize) {
    if let Some((blk, writes)) = lw.alt_joins.remove(&i) {
        lw.bcx.ins().jump(blk, &[]);
        lw.bcx.switch_to_block(blk);
        lw.bcx.seal_block(blk);
        for w in writes {
            lw.known_int[w as usize] = None;
        }
    }
}
