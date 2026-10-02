use super::*;

/// The chunk's basic blocks.
pub(super) struct ChunkCfg {
    pub(super) bb_pcs: Vec<usize>,
    pub(super) num_bbs: usize,
    pub(super) pc_to_bb: Vec<usize>,
    pub(super) bb_predecessors: Vec<Vec<usize>>,
}

pub(super) fn build_cfg(c: ChunkIn<'_>, scan: &ChunkScan) -> Option<ChunkCfg> {
    let ChunkIn { code, n, .. } = c;
    let ChunkScan {
        bb_starts,
        for_loops,
        ..
    } = scan;
    // BB-level dataflow for "is this register a
    // table at the use site". A blanket
    // `has_new_table && has_conditional → bail` safety
    // net would be sound but would reject the make-style
    // pattern where both branches of an Op::Eq + Jmp split
    // independently `NewTable R[A]` and then SetList into it.
    //
    // Forward dataflow:
    //   entry[BB] = intersection of exit[pred] for each predecessor
    //   exit[BB] = apply ops in BB body forward from entry[BB]
    //   entry[BB 0] = function params marked true (caller guarantee)
    //
    // After convergence we re-walk every PC; at each
    // SetTable / SetList / GetI / Len / Move-from-table, derive
    // the local state from `entry[bb]` + body-apply up to PC and
    // verify the relevant register is in the table-defined set.
    //
    // Move propagation is included so e.g. `local t = {}` (R[0])
    // followed by `Move R[5] = R[0]` and then SetTable R[5][...]
    // works — the existing `table_alloc_10k` pattern.
    let bb_pcs: Vec<usize> = (0..n)
        .filter(|&p| bb_starts.get(p).copied().unwrap_or(false))
        .collect();
    let num_bbs = bb_pcs.len();
    if num_bbs == 0 {
        return None;
    }
    let mut pc_to_bb: Vec<usize> = vec![0; n];
    for (idx, &start) in bb_pcs.iter().enumerate() {
        let end = bb_pcs.get(idx + 1).copied().unwrap_or(n);
        for p in start..end {
            pc_to_bb[p] = idx;
        }
    }

    // Build successors per BB via op-level semantics. Returns
    // (terminator-found, successor-bb-indices).
    let mut bb_successors: Vec<Vec<usize>> = vec![Vec::new(); num_bbs];
    for bb_idx in 0..num_bbs {
        let bb_start = bb_pcs[bb_idx];
        let bb_end = bb_pcs.get(bb_idx + 1).copied().unwrap_or(n);
        let mut found_terminator = false;
        let mut p = bb_start;
        while p < bb_end {
            let ins = code[p];
            match ins.op() {
                Op::Jmp => {
                    let tgt = jmp_target(p, ins);
                    if tgt < n {
                        let s = pc_to_bb[tgt];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    found_terminator = true;
                    break;
                }
                Op::Lt | Op::Le | Op::Eq => {
                    // Paired with the next op (always Jmp per scan).
                    let jmp = code[p + 1];
                    let tgt = jmp_target(p + 1, jmp);
                    if tgt < n {
                        let s = pc_to_bb[tgt];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    let fall = p + 2;
                    if fall < n {
                        let s = pc_to_bb[fall];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    found_terminator = true;
                    break;
                }
                Op::Return0 | Op::Return1 => {
                    found_terminator = true;
                    break;
                }
                Op::ForPrep => {
                    let fall = p + 1;
                    if fall < n {
                        let s = pc_to_bb[fall];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    if let Some(&(_, lp, _)) = for_loops.iter().find(|&&(pp, _, _)| pp == p) {
                        let exit_pc = lp + 1;
                        if exit_pc < n {
                            let s = pc_to_bb[exit_pc];
                            if !bb_successors[bb_idx].contains(&s) {
                                bb_successors[bb_idx].push(s);
                            }
                        }
                    }
                    found_terminator = true;
                    break;
                }
                Op::ForLoop => {
                    let exit_pc = p + 1;
                    if exit_pc < n {
                        let s = pc_to_bb[exit_pc];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    if let Some(&(prep, _, _)) = for_loops.iter().find(|&&(_, lp, _)| lp == p) {
                        let body = prep + 1;
                        if body < n {
                            let s = pc_to_bb[body];
                            if !bb_successors[bb_idx].contains(&s) {
                                bb_successors[bb_idx].push(s);
                            }
                        }
                    }
                    found_terminator = true;
                    break;
                }
                _ => {
                    p += 1;
                }
            }
        }
        if !found_terminator && bb_end < n {
            let s = pc_to_bb[bb_end];
            if !bb_successors[bb_idx].contains(&s) {
                bb_successors[bb_idx].push(s);
            }
        }
    }

    let mut bb_predecessors: Vec<Vec<usize>> = vec![Vec::new(); num_bbs];
    for src in 0..num_bbs {
        for &dst in &bb_successors[src] {
            if !bb_predecessors[dst].contains(&src) {
                bb_predecessors[dst].push(src);
            }
        }
    }
    Some(ChunkCfg {
        bb_pcs,
        num_bbs,
        pc_to_bb,
        bb_predecessors,
    })
}
