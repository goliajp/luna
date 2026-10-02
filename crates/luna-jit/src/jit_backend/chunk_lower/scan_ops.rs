use super::*;

pub(super) fn scan_calls(s: &mut ChunkScan, c: ChunkIn<'_>, pc: usize, ins: Inst) -> Option<()> {
    let ChunkIn {
        proto,
        code,
        n,
        num_params,
        float_only,
        ..
    } = c;
    let allows_self_recursion = s.allows_self_recursion;
    let ChunkScan {
        self_upval_idx,
        self_upval,
        is_upval_value_read,
        self_call_pcs,
        step_const,
        defines_table,
        folded_math,
        ..
    } = s;
    match ins.op() {
        Op::GetUpval => {
            let b = ins.b();
            if (b as usize) >= proto.upvals.len() {
                return None;
            }
            // ValueRead role: `R[A]` is consumed as
            // a real value (not a self-recursion call target).
            // For now we restrict to **Float-only dialects**
            // (5.1/5.2) so we can default-pin the upvalue's
            // runtime type to Float without a tag check. 5.3+
            // has Int subtype — the upvalue could be Int at
            // runtime; a Float interpretation would garble the
            // raw bits. 5.3+ would need a tag check + deopt path;
            // deferred. `pre53 && float_only` ⇔ 5.1 or 5.2.
            if is_upval_value_read[pc] {
                if !float_only {
                    return None;
                }
                // Don't tag `self_upval` — this GetUpval feeds an
                // arith/cmp/Return reader. Clear ancillary trackers
                // mirror-style at R[A].
                if let Some(slot) = step_const.get_mut(ins.a() as usize) {
                    *slot = None;
                }
                if let Some(slot) = defines_table.get_mut(ins.a() as usize) {
                    *slot = false;
                }
                return Some(());
            }
            // SelfMarker — self-recursion call target.
            if !allows_self_recursion {
                return None;
            }
            // Pin self-upval idx on first GetUpval; reject any
            // subsequent GetUpval that reads a different slot.
            // This is dialect-agnostic — Lua 5.5/5.4/5.3/5.2 fib
            // reads upvals[0], Lua 5.1 fib reads upvals[1] (with
            // upvals[0] being an unused `_ENV` placeholder).
            match *self_upval_idx {
                Some(idx) if idx != b => return None,
                Some(_) => {}
                None => *self_upval_idx = Some(b),
            }
            if let Some(slot) = self_upval.get_mut(ins.a() as usize) {
                *slot = true;
            }
            if let Some(slot) = step_const.get_mut(ins.a() as usize) {
                *slot = None;
            }
        }
        Op::Call => {
            let a = ins.a() as usize;
            // nargs / nresults bounds (apply to both self-recursive
            // and math-fold variants — `MathFold` already pins B=2
            // C=2, well within MAX_JIT_ARITY).
            //
            let nargs = ins.b().checked_sub(1)?;
            let c = ins.c();
            // variadic Call (C=0) paired with a
            // variadic SetList (B=0) at PC+1 is the
            // `{make(d-1), make(d-1)}` shape — luna's frontend
            // emits the second sibling's `Call` as variadic so
            // it can splat into the next SetList. Every JIT'd
            // chunk has `returns_one == true`, so the variadic
            // count is statically 1 and the SetList's implied
            // length is `A_call - A_list` (computed on the
            // SetList side).
            let next_is_variadic_setlist = c == 0
                && pc + 1 < n
                && matches!(code[pc + 1].op(), Op::SetList)
                && code[pc + 1].b() == 0;
            let nresults = if next_is_variadic_setlist {
                1
            } else {
                c.checked_sub(1)?
            };
            if nargs > MAX_JIT_ARITY as u32 || nresults != 1 {
                return None;
            }
            if folded_math[pc] {
                // math libcall fold. Emit-side folds the
                // 4-op window into one cranelift libm call; here
                // we just clear the per-register trackers.
            } else if self_upval.get(a).copied().unwrap_or(false) && nargs as usize == num_params {
                // self-recursive call, lowered as a direct
                // call of the compiled body, whose signature takes
                // exactly the function's parameters. The upvalue may
                // hold another function (the entry check catches that
                // at run time), so the call site's count can differ;
                // such a call is not lowered.
                self_call_pcs[pc] = true;
            } else {
                return None;
            }
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
        }
        Op::GetTabUp | Op::GetField => {
            // accepted only as part of a recognized
            // math libcall fold. The fold's emit consumes all
            // four PCs; the per-register trackers for R[A] get
            // cleared so post-fold uses see fresh state.
            if !folded_math[pc] {
                return None;
            }
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

/// Returns the pc the scan continues from (a comparison consumes its
/// paired `Jmp`).
pub(super) fn scan_control(
    s: &mut ChunkScan,
    c: ChunkIn<'_>,
    pc: usize,
    ins: Inst,
) -> Option<usize> {
    let ChunkIn { code, n, .. } = c;
    let ChunkScan {
        bb_starts,
        sees_return1,
        self_upval,
        step_const,
        for_loops,
        ..
    } = s;
    let mut pc = pc;
    match ins.op() {
        Op::Return1 => {
            // A Return1 of a self-upval-tagged register would return
            // the (mismarked) closure value back to the caller — not
            // a shape the lowerer handles. Bail.
            if self_upval.get(ins.a() as usize).copied().unwrap_or(false) {
                return None;
            }
            *sees_return1 = true;
            if pc + 1 < n {
                bb_starts[pc + 1] = true;
            }
        }
        Op::Return0 => {
            if pc + 1 < n {
                bb_starts[pc + 1] = true;
            }
        }
        Op::Jmp => {
            let tgt = jmp_target(pc, ins);
            // a jump to itself (`while true do end`, `::l:: goto l`)
            // would spin in native code, where the interpreter's
            // instruction budget and hooks never run
            if tgt >= n || tgt == pc {
                return None;
            }
            bb_starts[tgt] = true;
            if pc + 1 < n {
                bb_starts[pc + 1] = true;
            }
        }
        Op::Lt | Op::Le | Op::Eq => {
            // Reading a tagged register here is a generic-upvalue
            // comparison (e.g. `if n_upval < 3 then …`) which the
            // lowerer doesn't model.
            if self_upval.get(ins.a() as usize).copied().unwrap_or(false)
                || self_upval.get(ins.b() as usize).copied().unwrap_or(false)
            {
                return None;
            }
            // A comparison op is always paired with a following Jmp
            // (PUC's `cond_skip` invariant). luna's compiler never
            // emits one without the other; if we see a lone Lt/Le/Eq
            // the proto is malformed for our purposes — bail out.
            let &jmp = code.get(pc + 1)?;
            if !matches!(jmp.op(), Op::Jmp) {
                return None;
            }
            let jmp_pc = pc + 1;
            let tgt = jmp_target(jmp_pc, jmp);
            if tgt >= n {
                return None;
            }
            bb_starts[tgt] = true;
            if jmp_pc + 1 < n {
                bb_starts[jmp_pc + 1] = true;
            }
            pc = jmp_pc; // outer pc += 1 below moves past the Jmp
        }
        Op::ForPrep => {
            // both forms admitted. The dialect-
            // specific shape is picked up in emit, gated by `pre53`.
            let a = ins.a() as usize;
            // The step has to be a compile-time-known `LoadI`
            // immediate. luna's bytecode emitter always materialises
            // numeric-for steps via a `LoadI` (`for i = 1, N do …` →
            // step register pre-loaded with `LoadI 1`). A non-Int
            // step (`for i = 1, N, x` where x is a variable) bails.
            let step_imm = step_const.get(a + 2).copied().flatten()?;
            if step_imm == 0 {
                return None;
            }
            // Pair with the matching ForLoop. luna's interpreter
            // executes `add_pc(bx - 1)` *after* the natural
            // `pc += 1` post-step, so the running pc lands on the
            // OP_FORLOOP at `prep_pc + bx`. See
            // `src/vm/exec.rs::for_prep` (post53 branch).
            let loop_pc = pc + ins.bx() as usize;
            if loop_pc >= n {
                return None;
            }
            let loop_ins = code[loop_pc];
            if !matches!(loop_ins.op(), Op::ForLoop) || loop_ins.a() as usize != a {
                return None;
            }
            // BB boundaries: ForPrep is its own block; body starts
            // at pc+1; the ForLoop sits in the body's tail block;
            // the exit lands at loop_pc+1.
            bb_starts[pc + 1] = true;
            if loop_pc + 1 < n {
                bb_starts[loop_pc + 1] = true;
            }
            bb_starts[loop_pc] = true; // ForLoop opens its own block.
            for_loops.push((pc, loop_pc, step_imm));
            // ForPrep writes R[A], R[A+1], R[A+2], R[A+3] — every
            // register's step_const tracker is stale after this.
            for off in 0..=3 {
                if let Some(slot) = step_const.get_mut(a + off) {
                    *slot = None;
                }
                if let Some(slot) = self_upval.get_mut(a + off) {
                    *slot = false;
                }
            }
        }
        Op::ForLoop => {
            // ForLoop alone (without a paired ForPrep earlier in
            // the for_loops list) is an orphan — luna's bytecode
            // emitter never produces that, so reject any ForLoop
            // whose matching ForPrep wasn't recorded.
            let a = ins.a() as usize;
            if !for_loops.iter().any(|&(_, lp, _)| lp == pc) {
                return None;
            }
            // ForLoop writes R[A], R[A+1], R[A+3] on the continue
            // path — same step_const wipe as ForPrep.
            for off in [0usize, 1, 3] {
                if let Some(slot) = step_const.get_mut(a + off) {
                    *slot = None;
                }
                if let Some(slot) = self_upval.get_mut(a + off) {
                    *slot = false;
                }
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(pc)
}

pub(super) fn scan_tables(s: &mut ChunkScan, c: ChunkIn<'_>, pc: usize, ins: Inst) -> Option<()> {
    let ChunkIn { code, .. } = c;
    let ChunkScan {
        self_upval,
        step_const,
        defines_table,
        ..
    } = s;
    match ins.op() {
        Op::NewTable => {
            // empty-table form. luna's frontend emits
            // NewTable a=A b=0 c=0 for `{}`.
            // Also accept `b > 0` (array presize for
            // `{...}` literals); the emit-side calls
            // `luna_jit_new_table_sized(b)`. `c > 0` (hash part
            // presize) still bails — none of our headline cells
            // use hash literals, and the per-slot lowering would
            // need a separate dispatch for `nodes`.
            if ins.c() != 0 {
                return None;
            }
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
            if let Some(slot) = defines_table.get_mut(a) {
                *slot = true;
            }
        }
        Op::SetTable => {
            // register-keyed set. The proper safety
            // gate (R[A] must be a definitively-defined table at
            // this PC) lives in the BB-level dataflow check
            // below; the linear `defines_table` walk would
            // wrongly accept a false-branch-only NewTable.
        }
        Op::SetList => {
            // fixed-count array literal initializer
            // (B > 0). Variadic form (B == 0, C ==
            // 0) accepted when paired with the immediately
            // preceding `Op::Call C=0`; the JIT'd self-recursive
            // callee returns exactly 1 value, so the static
            // count is `A_call - A_list`.
            let b = ins.b();
            // the emit stores from index 1: no offset, and no
            // `ExtraArg` offset either
            if ins.c() != 0 || ins.k() {
                return None;
            }
            if b == 0 {
                if pc == 0 {
                    return None;
                }
                let prev = code[pc - 1];
                if !matches!(prev.op(), Op::Call) || prev.c() != 0 {
                    return None;
                }
                let a_call = prev.a() as i64;
                let a_list = ins.a() as i64;
                if a_call <= a_list {
                    return None;
                }
            }
            // BB-level dataflow verifies R[A] is a table at this
            // PC. No register-tracker side effects — SetList
            // writes through R[A] into the table's array part,
            // not into R[A..A+B] themselves.
        }
        Op::GetI => {
            // `R[A] = R[B][imm(C)]`. BB-level dataflow
            // verifies R[B] is a table at this PC.
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
            // R[A] receives an Int value pulled from the table;
            // it is not itself a table reference.
            if let Some(slot) = defines_table.get_mut(a) {
                *slot = false;
            }
        }
        Op::GetTable => {
            // `R[A] = R[B][R[C]]`. BB-level dataflow
            // verifies R[B] is a table at this PC. Parallel to
            // GetI but the key is in a register (5.1/5.2 lower
            // `t[1]` this way because they have no Int subtype:
            // the literal `1` lands in a register via `LoadF 1.0`
            // and then `OP_GETTABLE` reads it).
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
            if let Some(slot) = defines_table.get_mut(a) {
                *slot = false;
            }
        }
        Op::Len => {
            // `R[A] = #R[B]`. BB-level dataflow
            // verifies R[B] is a table at this PC.
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
            if let Some(slot) = defines_table.get_mut(a) {
                *slot = false;
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
