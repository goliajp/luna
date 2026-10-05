use super::*;

/// For each pc some path reaches, the registers that may hold nil when
/// that op runs, on some path through the chunk's blocks (`None` for code
/// no path reaches). The kind sweep walks the code in pc
/// order, so a write it passes on the way (`r = i` inside an `if`) would
/// otherwise count as done on every path, and a register still nil on the
/// path that skips it would be read as the number 0.
///
/// A local starts nil (only parameters come in with values), `LoadNil`
/// makes registers nil, `Move` copies the source's state, and any other
/// write clears it. Blocks join with "may be nil on any incoming path".
pub(super) fn may_nil_by_pc(c: ChunkIn<'_>, cfg: &ChunkCfg) -> Vec<Option<Vec<bool>>> {
    let ChunkIn {
        code,
        n,
        num_params,
        max_stack,
        ..
    } = c;
    let step = |state: &mut Vec<bool>, ins: Inst| match ins.op() {
        Op::LoadNil => {
            let a = ins.a() as usize;
            for r in a..=a + ins.b() as usize {
                if let Some(s) = state.get_mut(r) {
                    *s = true;
                }
            }
        }
        Op::Move => {
            let (a, b) = (ins.a() as usize, ins.b() as usize);
            if a < state.len() && b < state.len() {
                state[a] = state[b];
            }
        }
        _ => {
            let (_, writes) = crate::jit_backend::trace::op_reads_writes(ins);
            for r in writes {
                if let Some(s) = state.get_mut(r as usize) {
                    *s = false;
                }
            }
        }
    };
    let bb_end = |bb: usize| cfg.bb_pcs.get(bb + 1).copied().unwrap_or(n);
    let mut entry: Vec<Option<Vec<bool>>> = vec![None; cfg.num_bbs];
    entry[0] = Some((0..max_stack).map(|r| r >= num_params).collect());
    let mut changed = true;
    while changed {
        changed = false;
        for bb in 0..cfg.num_bbs {
            let Some(mut state) = entry[bb].clone() else {
                continue;
            };
            for &ins in &code[cfg.bb_pcs[bb]..bb_end(bb)] {
                step(&mut state, ins);
            }
            // `bb_predecessors` holds the edges; push this block's exit to
            // every block it is a predecessor of
            for succ in 0..cfg.num_bbs {
                if !cfg.bb_predecessors[succ].contains(&bb) {
                    continue;
                }
                match &mut entry[succ] {
                    Some(e) => {
                        for (x, &y) in e.iter_mut().zip(&state) {
                            if y && !*x {
                                *x = true;
                                changed = true;
                            }
                        }
                    }
                    slot @ None => {
                        *slot = Some(state.clone());
                        changed = true;
                    }
                }
            }
        }
    }
    let mut out = vec![None; n];
    for bb in 0..cfg.num_bbs {
        let Some(mut state) = entry[bb].clone() else {
            continue;
        };
        for pc in cfg.bb_pcs[bb]..bb_end(bb) {
            out[pc] = Some(state.clone());
            step(&mut state, code[pc]);
        }
    }
    out
}
