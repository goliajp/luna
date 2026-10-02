use super::*;

pub(super) fn bb_entry_kinds(
    c: ChunkIn<'_>,
    scan: &ChunkScan,
    cfg: &ChunkCfg,
    reg_kinds: &[RegKind],
    ret_kind: RegKind,
    arg_float_mask: u8,
    arg_table_mask: u8,
) -> Vec<Vec<RegKind>> {
    let ChunkIn {
        num_params,
        max_stack,
        ..
    } = c;
    let num_bbs = cfg.num_bbs;
    let bb_predecessors = &cfg.bb_predecessors;
    // per-BB RegKind dataflow.
    //
    // `bb_entry_kinds[bb][r]` is the active kind (latest-writer kind on
    // every path reaching this BB) for register `r` at the BB's entry
    // PC. emit-time `current_kinds` resets to this on every BB switch
    // so an alternate-path writer's kind doesn't leak into the
    // current path. Readers gate behind this so Float-vs-Table
    // register reuse across BBs can unify globally (the 5.1/5.2
    // binary_trees + table_alloc shapes).
    //
    // `Op::SetList` reads regs it just wrote inside the same BB
    // (writers always immediately precede the SetList), so the reset
    // never changes SetList's view.
    //
    // Lattice:
    //   TOP = `RegKind::Unset` (initial non-entry BB entry; encodes
    //         "no info yet" during fixpoint and "fall back to declared
    //         `reg_kinds`" at emit time).
    //   `Int` / `Float` / `Table` = definite kinds.
    //   meet(X, X) = X; meet(X, Unset) = X; meet(X, Y) for X ≠ Y =
    //   Unset (join conflict — emit-side readers fall back).
    //
    // Mirrors the `defines_table` dataflow shape: forward, fixed
    // point with intersection-at-joins, non-entry BBs init at TOP, BB
    // 0 init from param kinds.
    let init_kind_for_reg = |i: usize| -> RegKind {
        if i < num_params {
            if (arg_float_mask >> i) & 1 == 1 {
                RegKind::Float
            } else if (arg_table_mask >> i) & 1 == 1 {
                RegKind::Table
            } else {
                RegKind::Int
            }
        } else {
            RegKind::Unset
        }
    };
    let meet_kind = |a: RegKind, b: RegKind| -> RegKind {
        match (a, b) {
            (RegKind::Unset, x) | (x, RegKind::Unset) => x,
            (x, y) if x == y => x,
            _ => RegKind::Unset,
        }
    };

    let mut bb_entry_kinds: Vec<Vec<RegKind>> = (0..num_bbs)
        .map(|_| vec![RegKind::Unset; max_stack])
        .collect();
    let mut bb_exit_kinds: Vec<Vec<RegKind>> = (0..num_bbs)
        .map(|_| vec![RegKind::Unset; max_stack])
        .collect();
    for i in 0..max_stack {
        bb_entry_kinds[0][i] = init_kind_for_reg(i);
    }
    let max_iters_kinds = num_bbs * (max_stack + 2);
    let mut iters_kinds = 0;
    let mut changed_kinds = true;
    while changed_kinds && iters_kinds < max_iters_kinds {
        changed_kinds = false;
        iters_kinds += 1;
        for bb_idx in 0..num_bbs {
            let new_entry = if bb_predecessors[bb_idx].is_empty() {
                bb_entry_kinds[bb_idx].clone()
            } else {
                let mut e = bb_exit_kinds[bb_predecessors[bb_idx][0]].clone();
                for &pred in &bb_predecessors[bb_idx][1..] {
                    for (i, val) in bb_exit_kinds[pred].iter().enumerate() {
                        e[i] = meet_kind(e[i], *val);
                    }
                }
                if bb_idx == 0 {
                    for i in 0..max_stack {
                        e[i] = init_kind_for_reg(i);
                    }
                }
                e
            };
            let mut state = new_entry.clone();
            apply_bb_kinds(c, scan, cfg, reg_kinds, ret_kind, bb_idx, &mut state);
            if state != bb_exit_kinds[bb_idx] {
                bb_exit_kinds[bb_idx] = state;
                changed_kinds = true;
            }
            if new_entry != bb_entry_kinds[bb_idx] {
                bb_entry_kinds[bb_idx] = new_entry;
                changed_kinds = true;
            }
        }
    }
    bb_entry_kinds
}

fn apply_bb_kinds(
    c: ChunkIn<'_>,
    scan: &ChunkScan,
    cfg: &ChunkCfg,
    reg_kinds: &[RegKind],
    ret_kind: RegKind,
    bb_idx: usize,
    state: &mut [RegKind],
) {
    let ChunkIn {
        proto,
        code,
        n,
        pre53,
        ..
    } = c;
    let ChunkScan {
        folded_math,
        math_folds,
        ..
    } = scan;
    let bb_pcs = &cfg.bb_pcs;
    let bb_start = bb_pcs[bb_idx];
    let bb_end = bb_pcs.get(bb_idx + 1).copied().unwrap_or(n);
    for p in bb_start..bb_end {
        let ins = code[p];
        // Math fold: the underlying GetField / Move / Call inside
        // a fold are skipped at emit (`pc += 3` after the
        // GetTabUp), so their would-be writes don't happen. Only
        // the GetTabUp at `start_pc` actually writes
        // `fold.dst_reg = Float`.
        if folded_math[p] {
            if let Some(fold) = math_folds.iter().find(|f| f.start_pc == p)
                && let Some(slot) = state.get_mut(fold.dst_reg as usize)
            {
                *slot = fold.result_kind();
            }
            continue;
        }
        match ins.op() {
            Op::LoadI => {
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = RegKind::Int;
                }
            }
            Op::LoadF => {
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = RegKind::Float;
                }
            }
            Op::LoadK => {
                let k = match proto.consts.get(ins.bx() as usize) {
                    Some(LuaValue::Float(_)) => RegKind::Float,
                    Some(LuaValue::Int(_)) => RegKind::Int,
                    _ => RegKind::Unset,
                };
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = k;
                }
            }
            Op::Move => {
                let src_kind = state
                    .get(ins.b() as usize)
                    .copied()
                    .unwrap_or(RegKind::Unset);
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = src_kind;
                }
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div => {
                // Result kind is picked from the sweep's
                // `reg_kinds[a]` at emit (`current_kinds[a] = k`
                // mirrors that). Replay the same here.
                let k = reg_kinds
                    .get(ins.a() as usize)
                    .copied()
                    .unwrap_or(RegKind::Unset);
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = k;
                }
            }
            Op::Call => {
                // Self-recursive (the only non-folded Call shape
                // the whitelist admits). Result is `ret_kind`.
                if !matches!(ret_kind, RegKind::Unset)
                    && let Some(slot) = state.get_mut(ins.a() as usize)
                {
                    *slot = ret_kind;
                }
            }
            Op::ForPrep | Op::ForLoop => apply_for_kinds(pre53, reg_kinds, ins, state),
            Op::NewTable => {
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = RegKind::Table;
                }
            }
            Op::GetI | Op::GetTable => {
                // Emit writes `current_kinds[a] = reg_kinds[a]`
                // (the declared kind picked by the sweep, since
                // GetI/GetTable's helper returns raw payload
                // bits that could be Int, Float or Table at
                // runtime — the sweep + `maybe_table` tracker
                // handles the ambiguity downstream).
                let k = reg_kinds
                    .get(ins.a() as usize)
                    .copied()
                    .unwrap_or(RegKind::Unset);
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = k;
                }
            }
            Op::LoadNil => {
                // emit writes iconst(0) into each
                // `R[A..=A+B]` slot. The declared kind (Int by
                // the sweep's Unset→Int default, or whatever a
                // prior writer pinned) stays. The emit-side
                // `current_is_nil` shadow (reset at every BB
                // switch, set true here, cleared by other emit
                // writers) is the SetList disambiguation signal.
                let k = reg_kinds
                    .get(ins.a() as usize)
                    .copied()
                    .unwrap_or(RegKind::Int);
                let a = ins.a() as usize;
                for off in 0..=(ins.b() as usize) {
                    if let Some(slot) = state.get_mut(a + off) {
                        *slot = k;
                    }
                }
            }
            Op::Len => {
                if let Some(slot) = state.get_mut(ins.a() as usize) {
                    *slot = RegKind::Int;
                }
            }
            // GetUpval emits a placeholder def_var(0) but does
            // not update `current_kinds` (the matching Call
            // reads `reg_kinds`, not `current_kinds`). Mirror
            // that here — no state change.
            // SetTable / SetList write through R[A]; R[A] stays
            // whatever it was.
            _ => {}
        }
    }
}

fn apply_for_kinds(pre53: bool, reg_kinds: &[RegKind], ins: Inst, state: &mut [RegKind]) {
    match ins.op() {
        Op::ForPrep => {
            let a = ins.a() as usize;
            let is_float = matches!(
                reg_kinds.get(a).copied().unwrap_or(RegKind::Unset),
                RegKind::Float
            );
            match (pre53, is_float) {
                (true, false) => {
                    if let Some(s) = state.get_mut(a) {
                        *s = RegKind::Int;
                    }
                    if let Some(s) = state.get_mut(a + 1) {
                        *s = RegKind::Int;
                    }
                    if let Some(s) = state.get_mut(a + 2) {
                        *s = RegKind::Int;
                    }
                }
                (false, false) => {
                    if let Some(s) = state.get_mut(a) {
                        *s = RegKind::Int;
                    }
                    if let Some(s) = state.get_mut(a + 1) {
                        *s = RegKind::Int;
                    }
                    if let Some(s) = state.get_mut(a + 2) {
                        *s = RegKind::Int;
                    }
                    if let Some(s) = state.get_mut(a + 3) {
                        *s = RegKind::Int;
                    }
                }
                (true, true) => {
                    if let Some(s) = state.get_mut(a) {
                        *s = RegKind::Float;
                    }
                    if let Some(s) = state.get_mut(a + 1) {
                        *s = RegKind::Float;
                    }
                    if let Some(s) = state.get_mut(a + 2) {
                        *s = RegKind::Int;
                    }
                }
                (false, true) => {
                    if let Some(s) = state.get_mut(a) {
                        *s = RegKind::Float;
                    }
                    if let Some(s) = state.get_mut(a + 1) {
                        *s = RegKind::Float;
                    }
                    if let Some(s) = state.get_mut(a + 2) {
                        *s = RegKind::Int;
                    }
                    if let Some(s) = state.get_mut(a + 3) {
                        *s = RegKind::Float;
                    }
                }
            }
        }
        Op::ForLoop => {
            let a = ins.a() as usize;
            let is_float = matches!(
                reg_kinds.get(a).copied().unwrap_or(RegKind::Unset),
                RegKind::Float
            );
            if is_float {
                if let Some(s) = state.get_mut(a) {
                    *s = RegKind::Float;
                }
                if let Some(s) = state.get_mut(a + 3) {
                    *s = RegKind::Float;
                }
            } else if pre53 {
                if let Some(s) = state.get_mut(a) {
                    *s = RegKind::Int;
                }
                if let Some(s) = state.get_mut(a + 3) {
                    *s = RegKind::Int;
                }
            } else {
                if let Some(s) = state.get_mut(a) {
                    *s = RegKind::Int;
                }
                if let Some(s) = state.get_mut(a + 1) {
                    *s = RegKind::Int;
                }
                if let Some(s) = state.get_mut(a + 3) {
                    *s = RegKind::Int;
                }
            }
        }
        _ => unreachable!("dispatched by op"),
    }
}
