use super::*;

// Body-apply: forward semantics for one BB's body, mutating `state`.
fn body_apply(c: ChunkIn<'_>, cfg: &ChunkCfg, bb_idx: usize, state: &mut [bool]) {
    let ChunkIn { code, n, .. } = c;
    let bb_pcs = &cfg.bb_pcs;
    let bb_start = bb_pcs[bb_idx];
    let bb_end = bb_pcs.get(bb_idx + 1).copied().unwrap_or(n);
    for p in bb_start..bb_end {
        let ins = code[p];
        match ins.op() {
            Op::NewTable => {
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = true;
                }
            }
            Op::Move => {
                let a = ins.a() as usize;
                let b = ins.b() as usize;
                let src_def = state.get(b).copied().unwrap_or(false);
                if let Some(slot) = state.get_mut(a) {
                    *slot = src_def;
                }
            }
            Op::GetI | Op::GetTable | Op::Len => {
                // Result is Int — not a table.
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = false;
                }
            }
            Op::LoadI | Op::LoadF | Op::LoadK | Op::Add | Op::Sub | Op::Mul | Op::Div => {
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = false;
                }
            }
            Op::LoadNil => {
                // LoadNil writes Nil to R[A..=A+B];
                // none of those are table refs.
                let a = ins.a() as usize;
                for off in 0..=(ins.b() as usize) {
                    if let Some(slot) = state.get_mut(a + off) {
                        *slot = false;
                    }
                }
            }
            Op::Call => {
                // Self-recursive (the only Call shape the scan
                // admits outside the math fold) may return a
                // table when `ret_kind` is Table — but the kind
                // sweep that decides that hasn't run yet at this
                // point in the pass. Treat conservatively: clear
                // the bit. RegKind sweep + emit will catch any
                // mismatch as a unify failure / IR-time bail.
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = false;
                }
            }
            Op::GetUpval | Op::GetTabUp | Op::GetField => {
                // None of these produce a table-valued result in
                // the current whitelist (math fold's GetTabUp /
                // GetField are consumed in-line).
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = false;
                }
            }
            Op::ForPrep | Op::ForLoop => {
                let a = ins.a() as usize;
                for off in 0..=3 {
                    if let Some(slot) = state.get_mut(a + off) {
                        *slot = false;
                    }
                }
            }
            // SetTable / SetList write *through* R[A]; the table
            // ref itself stays whatever it was.
            _ => {}
        }
    }
}

pub(super) fn check_table_operands(c: ChunkIn<'_>, cfg: &ChunkCfg) -> Option<()> {
    let ChunkIn {
        code,
        n,
        num_params,
        max_stack,
        ..
    } = c;
    let num_bbs = cfg.num_bbs;
    let ChunkCfg {
        bb_pcs,
        pc_to_bb,
        bb_predecessors,
        ..
    } = cfg;
    // "must-defined" dataflow uses intersection at
    // joins, so we initialise non-entry BBs at the TOP element
    // (every register considered defined) and refine downward.
    // Starting at BOTTOM (false) would make the intersection at
    // any back-edge converge to false immediately.
    let mut bb_entry: Vec<Vec<bool>> = (0..num_bbs).map(|i| vec![i != 0; max_stack]).collect();
    let mut bb_exit: Vec<Vec<bool>> = vec![vec![true; max_stack]; num_bbs];
    // Entry BB starts with params marked as defined (caller guarantee
    // mirrors the linear walk's init above).
    for i in 0..max_stack {
        if let Some(slot) = bb_entry[0].get_mut(i) {
            *slot = i < num_params;
        }
    }
    let mut iters = 0;
    let max_iters = num_bbs * (max_stack + 2);
    let mut changed = true;
    while changed && iters < max_iters {
        changed = false;
        iters += 1;
        for bb_idx in 0..num_bbs {
            let new_entry = if bb_predecessors[bb_idx].is_empty() {
                // Unreachable BB or BB 0. Keep existing entry (params
                // marked at start for BB 0; all-false for others).
                bb_entry[bb_idx].clone()
            } else {
                let mut e = bb_exit[bb_predecessors[bb_idx][0]].clone();
                for &pred in &bb_predecessors[bb_idx][1..] {
                    for (i, val) in bb_exit[pred].iter().enumerate() {
                        e[i] &= val;
                    }
                }
                if bb_idx == 0 {
                    for i in 0..num_params {
                        if let Some(slot) = e.get_mut(i) {
                            *slot = true;
                        }
                    }
                }
                e
            };
            let mut state = new_entry.clone();
            body_apply(c, cfg, bb_idx, &mut state);
            if state != bb_exit[bb_idx] {
                bb_exit[bb_idx] = state;
                changed = true;
            }
            if new_entry != bb_entry[bb_idx] {
                bb_entry[bb_idx] = new_entry;
                changed = true;
            }
        }
    }

    // Per-use BB-level safety check.
    for p in 0..n {
        let ins = code[p];
        let check_reg = match ins.op() {
            Op::SetTable | Op::SetList => Some(ins.a() as usize),
            Op::GetI | Op::GetTable | Op::Len => Some(ins.b() as usize),
            _ => None,
        };
        if let Some(reg) = check_reg {
            let bb_idx = pc_to_bb[p];
            let bb_start = bb_pcs[bb_idx];
            let mut state = bb_entry[bb_idx].clone();
            // Apply ops up to (but not including) p.
            for q in bb_start..p {
                let prev = code[q];
                match prev.op() {
                    Op::NewTable => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = true;
                        }
                    }
                    Op::Move => {
                        let a = prev.a() as usize;
                        let b = prev.b() as usize;
                        let src_def = state.get(b).copied().unwrap_or(false);
                        if let Some(slot) = state.get_mut(a) {
                            *slot = src_def;
                        }
                    }
                    Op::GetI | Op::GetTable | Op::Len => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = false;
                        }
                    }
                    Op::LoadI | Op::LoadF | Op::LoadK | Op::Add | Op::Sub | Op::Mul | Op::Div => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = false;
                        }
                    }
                    Op::LoadNil => {
                        let a = prev.a() as usize;
                        for off in 0..=(prev.b() as usize) {
                            if let Some(slot) = state.get_mut(a + off) {
                                *slot = false;
                            }
                        }
                    }
                    Op::Call => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = false;
                        }
                    }
                    Op::GetUpval | Op::GetTabUp | Op::GetField => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = false;
                        }
                    }
                    Op::ForPrep | Op::ForLoop => {
                        let a = prev.a() as usize;
                        for off in 0..=3 {
                            if let Some(slot) = state.get_mut(a + off) {
                                *slot = false;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !state.get(reg).copied().unwrap_or(false) {
                return None;
            }
        }
    }
    Some(())
}
