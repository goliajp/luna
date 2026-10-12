//! The scan of table ops.

use super::*;

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
            // a table with no hash part (`{}` or a list literal); one
            // whose constructor sizes a hash part still bails — none of
            // our headline cells use hash literals, and the per-slot
            // lowering would need a separate dispatch for `nodes`
            match luna_core::runtime::table::new_table_sizes(ins.b(), ins.c(), ins.k()) {
                Some((_, 0)) => {}
                _ => return None,
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
