use super::*;

pub(super) fn presize_hints(
    c: ChunkIn<'_>,
    scan: &ChunkScan,
) -> std::collections::HashMap<usize, i64> {
    let ChunkIn { proto, code, .. } = c;
    let for_loops = &scan.for_loops;
    // per-PC presize hint for `Op::NewTable`. When a
    // NewTable is immediately followed by the canonical
    // `LoadI init / LoadI/LoadK limit / LoadI step / ForPrep`
    // window with `init = 1`, `step = 1`, `limit = N` (Int const),
    // emit reaches for `luna_jit_new_table_sized(N)` to skip the
    // table-fill loop's intermediate rehashes. Map is sparse —
    // only NewTables that match the pattern get an entry. Filled
    // by a second scan pass below (the main whitelist pass already
    // produces `for_loops`, which gives us the matching ForPrep
    // PCs cheaply).
    let mut presize_for_newtable: std::collections::HashMap<usize, i64> =
        std::collections::HashMap::new();

    // find every NewTable that opens a
    // `NewTable R[A]=`{}`; LoadI R[A+1]=1; LoadI|LoadK R[A+2]=N;
    // LoadI R[A+3]=1; ForPrep R[A+1]` window. The matching ForPrep
    // is already in `for_loops`; we walk that list and look 4 PCs
    // back. Sizing hint = N (the `limit` const, at the third op of
    // the window). Bench source `for i = 1, 10000 do t[i] = i end`
    // matches; arbitrary loop bodies after ForPrep don't affect the
    // pattern (we only inspect the four ops between NewTable and
    // ForPrep, inclusive).
    for &(prep_pc, loop_pc, step_imm) in for_loops {
        if step_imm != 1 || prep_pc < 4 {
            continue;
        }
        let nt_pc = prep_pc - 4;
        let init_pc = prep_pc - 3;
        let limit_pc = prep_pc - 2;
        let step_pc = prep_pc - 1;

        let nt = code[nt_pc];
        let init = code[init_pc];
        let limit = code[limit_pc];
        let step = code[step_pc];
        let fp = code[prep_pc];

        if !matches!(nt.op(), Op::NewTable) {
            continue;
        }
        if nt.b() != 0 || nt.c() != 0 {
            continue;
        }
        let fp_base = fp.a() as i64;
        if (nt.a() as i64) + 1 != fp_base {
            continue;
        }
        // R[A+1] = init = LoadI 1.
        if !matches!(init.op(), Op::LoadI) || init.a() as i64 != fp_base || init.sbx() != 1 {
            continue;
        }
        // R[A+2] = limit = LoadI or LoadK Int. sbx fits in i32; we
        // already clamp at the helper.
        let limit_val: i64 = match limit.op() {
            Op::LoadI if limit.a() as i64 == fp_base + 1 => limit.sbx() as i64,
            Op::LoadK if limit.a() as i64 == fp_base + 1 => {
                let bx = limit.bx() as usize;
                match proto.consts.get(bx).copied() {
                    Some(LuaValue::Int(v)) => v,
                    _ => continue,
                }
            }
            _ => continue,
        };
        // R[A+3] = step = LoadI 1.
        if !matches!(step.op(), Op::LoadI) || step.a() as i64 != fp_base + 2 || step.sbx() != 1 {
            continue;
        }
        if limit_val <= 0 || limit_val > (1 << 27) {
            continue;
        }
        if !fills_every_slot(proto, &code[prep_pc + 1..loop_pc], nt.a(), fp.a() + 3) {
            continue;
        }
        // filling `1..=N` in order leaves a table PUC sized by doubling:
        // an array part of the next power of two, no hash part. Sizing it
        // so at once is the same table, and nothing in the body sees the
        // difference on the way
        presize_for_newtable.insert(
            nt_pc,
            pow2_array_ops((limit_val as u64).next_power_of_two()),
        );
    }
    presize_for_newtable
}

/// Packed `NewTable` operands (see `pack_table_ops`) for an array part of
/// `n`, a power of two, and no hash part: the 5.1–5.3 floating point byte
/// form, which holds any power of two exactly.
fn pow2_array_ops(n: u64) -> i64 {
    debug_assert!(n.is_power_of_two());
    let m = n.trailing_zeros() as i64;
    let fb = if n < 8 { n as i64 } else { (m - 2) << 3 };
    fb | 1 << 16
}

/// Whether a loop body only stores a non-nil value at `t[ivar]`: either
/// `t[i] = i`, or `t[i] = k` with `k` a number loaded just before. Nothing
/// else runs, so the body cannot fail or look at `t` part way through.
fn fills_every_slot(proto: Gc<Proto>, body: &[Inst], t: u32, ivar: u32) -> bool {
    let store = |i: &Inst, v: u32| {
        i.op() == Op::SetTable && i.a() == t && i.b() == ivar && i.c() == v && !i.k()
    };
    match body {
        [st] => store(st, ivar),
        [ld, st] if ld.a() != t && ld.a() != ivar => {
            let number = match ld.op() {
                Op::LoadI | Op::LoadF => true,
                Op::LoadK => matches!(
                    proto.consts.get(ld.bx() as usize),
                    Some(LuaValue::Int(_) | LuaValue::Float(_))
                ),
                _ => false,
            };
            number && store(st, ld.a())
        }
        _ => false,
    }
}

pub(super) fn check_fold_blocks(scan: &ChunkScan) -> Option<()> {
    let ChunkScan {
        bb_starts,
        math_folds,
        ..
    } = scan;
    // every math fold's internal PCs (+1, +2, +3) must
    // sit inside a single basic block. A Jmp target landing on one
    // of them would leave a half-emitted fold straddling a Cranelift
    // block boundary (the BB algorithm marks the target as a block
    // start but emit's `pc += 3` jumps over it without visiting).
    // luna's frontend never produces such a jump, but bail
    // defensively to keep the IR well-formed.
    for fold in math_folds {
        for off in 1..=3 {
            if bb_starts.get(fold.start_pc + off).copied().unwrap_or(false) {
                return None;
            }
        }
    }
    Some(())
}

/// Returns whether the chunk makes a self-recursive call.
pub(super) fn check_self_call_base_case(c: ChunkIn<'_>, scan: &ChunkScan) -> Option<bool> {
    let ChunkIn { code, n, .. } = c;
    let ChunkScan {
        self_call_pcs,
        for_loops,
        ..
    } = scan;
    // Correctness gate: every JIT-recognised self-recursive call
    // bypasses luna's `c_depth` / `frames.len()` budget. A self-call
    // with no base case before it would blow the OS stack (the
    // `runtime_stack_overflow_is_caught` regression). Require at least
    // one Return reachable from PC 0 WITHOUT passing through a self-
    // recursive Call PC. fib has the early `if n < 2 then return n end`
    // path; `f() return 1 + f() end` has no such path and bails.
    let any_self_call = self_call_pcs.iter().any(|&b| b);
    if any_self_call {
        let mut visited = vec![false; n];
        let mut stack = vec![0usize];
        visited[0] = true;
        let mut safe_return_reached = false;
        while let Some(pc) = stack.pop() {
            let ins = code[pc];
            match ins.op() {
                Op::Return0 | Op::Return1 => {
                    safe_return_reached = true;
                    break;
                }
                Op::Jmp => {
                    let tgt = jmp_target(pc, ins);
                    if tgt < n && !visited[tgt] {
                        visited[tgt] = true;
                        stack.push(tgt);
                    }
                }
                Op::Lt | Op::Le | Op::Eq => {
                    // skip the paired Jmp's PC; consider both successors
                    let jmp = code[pc + 1];
                    let jmp_tgt = jmp_target(pc + 1, jmp);
                    if jmp_tgt < n && !visited[jmp_tgt] {
                        visited[jmp_tgt] = true;
                        stack.push(jmp_tgt);
                    }
                    let fall = pc + 2;
                    if fall < n && !visited[fall] {
                        visited[fall] = true;
                        stack.push(fall);
                    }
                }
                Op::Call if self_call_pcs[pc] => {
                    // self-recursive — treat as a wall; do NOT traverse past.
                }
                Op::ForPrep => {
                    // Two successors: fall-through (body) AND the
                    // paired ForLoop's exit (skip when empty). Either
                    // path can reach a Return.
                    let fall = pc + 1;
                    if fall < n && !visited[fall] {
                        visited[fall] = true;
                        stack.push(fall);
                    }
                    if let Some(&(_, lp, _)) = for_loops.iter().find(|&&(p, _, _)| p == pc) {
                        let exit = lp + 1;
                        if exit < n && !visited[exit] {
                            visited[exit] = true;
                            stack.push(exit);
                        }
                    }
                }
                _ => {
                    let fall = pc + 1;
                    if fall < n && !visited[fall] {
                        visited[fall] = true;
                        stack.push(fall);
                    }
                }
            }
        }
        if !safe_return_reached {
            return None;
        }
    }
    Some(any_self_call)
}
