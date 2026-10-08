//! The kinds a numeric `for` gives its registers.

use super::*;

pub(super) fn sweep_for(st: &mut KindSweep, ins: Inst) -> Option<()> {
    let KindSweep {
        reg_kinds,
        latest_writer_kind,
        maybe_table,
        is_nil_writer,
        ..
    } = st;
    match ins.op() {
        Op::ForPrep | Op::ForLoop | Op::ForPrep55 | Op::ForLoop55 => {
            // Int loop, or Float loop (5.1 /
            // 5.2 numeric `for` keeps the loop var Float). The
            // loop kind is decided by R[A]'s scanned kind: Float
            // at any pass forces Float for R[A], R[A+1], R[A+3]
            // (Unset / Int → Int path, the existing behaviour).
            // R[A+2] (step) is independent: PUC's numeric-for
            // compiler always emits an Int step immediate (LoadI
            // 1 / -1 / …), even in 5.1 / 5.2 Float loops, so we
            // pin it Int regardless and the Float emit promotes
            // the immediate to f64const at use sites.
            //
            // with the relaxed Int+Table `unify`
            // a `for i = 1, {}, 10 do … end` chunk's `limit`
            // slot (R[A+1]) holds a Table while `reg_kinds`
            // says Int. The interpreter raises "for limit
            // must be a number"; the JIT's `isub(ptr, 1)` /
            // `icmp` would silently compute a junk count and
            // exit cleanly, returning success where Lua
            // would have raised. Reject any of the four
            // loop slots being Table at the latest write,
            // including `maybe_table` (a GetI return).
            let a = ins.a() as usize;
            let r = ForRegs::of(ins);
            let loop_kind = match reg_kinds[a] {
                RegKind::Float => RegKind::Float,
                RegKind::Int | RegKind::Unset => RegKind::Int,
                RegKind::Table => return None,
            };
            // Likewise a nil-written init / limit / step (`for i =
            // 1, nil`, or a declared-uninitialized local): the
            // interpreter raises the 'for' error, the JIT would
            // loop over the Variable's zero payload.
            for off in 0..=r.var - a {
                if matches!(latest_writer_kind[a + off], RegKind::Table)
                    || maybe_table[a + off]
                    || (off < 3 && is_nil_writer[a + off])
                {
                    return None;
                }
            }
            for reg in [r.idx, r.x, r.var] {
                if !RegKind::unify(&mut reg_kinds[reg], loop_kind) {
                    return None;
                }
                latest_writer_kind[reg] = loop_kind;
                maybe_table[reg] = false;
                is_nil_writer[reg] = false;
            }
            // 5.1–5.4 keep the step in a register of its own, which holds
            // the integer immediate; 5.5 in the limit's, of the loop's kind
            let step_kind = if r.var == r.idx {
                loop_kind
            } else {
                RegKind::Int
            };
            if !RegKind::unify(&mut reg_kinds[r.step], step_kind) {
                return None;
            }
            latest_writer_kind[r.step] = step_kind;
            maybe_table[r.step] = false;
            is_nil_writer[r.step] = false;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
