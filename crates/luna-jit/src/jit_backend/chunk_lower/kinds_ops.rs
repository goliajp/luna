use super::*;

pub(super) fn sweep_calls(
    st: &mut KindSweep,
    scan: &ChunkScan,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let ChunkScan {
        is_upval_value_read,
        folded_math,
        math_folds,
        ..
    } = scan;
    let KindSweep {
        reg_kinds,
        ret_kind,
        latest_writer_kind,
        maybe_table,
        is_nil_writer,
    } = st;
    match ins.op() {
        Op::GetUpval => {
            // no kind constraint for the SelfMarker role.
            // The self-upval marker is never read as a real
            // value (the matching Op::Call rewrites to a
            // direct cranelift call, bypassing the register).
            // The Variable's declared type is decided by
            // whatever else reads R[A] around this — typically
            // a later same-register arith result whose kind we
            // already pinned. The emit-side `aligned_def`
            // makes the placeholder zero match whatever
            // declared type we picked.
            //
            // 5.2 fib hits this: R[1] is LoadF'd to Float, then
            // re-used by GetUpval(self), then Call writes the
            // Float self-result. Pinning Int here would conflict
            // with the LoadF and bail the whole Proto.
            //
            // ValueRead role: pin R[A] to Float so
            // downstream arith picks `fadd`/`fmul`. Restricted
            // to pre53 (linear pre-pass already bails non-pre53
            // value-read).
            if is_upval_value_read[pc] {
                let a = ins.a() as usize;
                if !RegKind::unify(&mut reg_kinds[a], RegKind::Float) {
                    return None;
                }
                latest_writer_kind[a] = RegKind::Float;
                maybe_table[a] = false;
                is_nil_writer[a] = false;
            }
        }
        Op::Call => {
            if folded_math[pc] {
                // pin R[A] (= Call.A = the fold's
                // result slot) to the fold's result kind.
                let k = math_folds
                    .iter()
                    .find(|f| f.start_pc + 3 == pc)
                    .map_or(RegKind::Float, MathFold::result_kind);
                if !RegKind::unify(&mut reg_kinds[ins.a() as usize], k) {
                    return None;
                }
                latest_writer_kind[ins.a() as usize] = k;
                maybe_table[ins.a() as usize] = false;
                is_nil_writer[ins.a() as usize] = false;
            } else {
                // Self-recursive call result kind = the Proto's
                // own ret kind.
                if !RegKind::unify(&mut reg_kinds[ins.a() as usize], *ret_kind) {
                    return None;
                }
                if !matches!(ret_kind, RegKind::Unset) {
                    latest_writer_kind[ins.a() as usize] = *ret_kind;
                }
                // The self-recursive callee's return kind is
                // statically known; clear any prior
                // maybe_table tag on R[A].
                maybe_table[ins.a() as usize] = false;
                is_nil_writer[ins.a() as usize] = false;
            }
        }
        Op::GetTabUp | Op::GetField => {
            // folded GetTabUp / GetField don't ever
            // observe their stored values (the next fold op
            // overwrites R[A]). The Call PC pins R[A] to
            // Float on its own; nothing to do here.
            if !folded_math[pc] {
                return None;
            }
        }
        Op::Return1 => {
            // Return1 on a LoadNil-written register
            // would wrap `Int(0)` instead of `Nil` (the helper
            // ABI is i64 bits; the dispatcher uses ret_kind to
            // decide Int vs Float, not Nil). Bail to interp so
            // a `function () return nil end` returns Nil, not
            // Int(0).
            if is_nil_writer[ins.a() as usize] {
                return None;
            }
            // pick from the most recent writer
            // instead of the unified `reg_kinds` slot so a
            // `LoadI 0 → Eq → NewTable → Return1` chain
            // sees the Return as a Table return (not Int).
            let a_kind = latest_writer_kind[ins.a() as usize];
            let a_kind = if matches!(a_kind, RegKind::Unset) {
                reg_kinds[ins.a() as usize]
            } else {
                a_kind
            };
            if !RegKind::unify(ret_kind, a_kind) {
                return None;
            }
            // Late: now that ret_kind may have been pinned,
            // back-propagate to R[A] so a Float ret pins the
            // register's type even when R[A] was Unset.
            //
            // guard on Unset: a 5.1/5.2
            // `LoadF + GetTable + Return1` chain reuses R[A]
            // as the Float-key holder before GetTable stores
            // the raw-payload result. `reg_kinds[a]` already
            // pinned Float by LoadF; we set `ret_kind = Int`
            // (the helper's raw-payload contract, latest
            // writer = Int). Unifying Float vs Int here would
            // bail the chunk needlessly — the Variable stays
            // F64, and the Return1 emit bitcasts the F64 use
            // back to I64 so the i64 bits ferry through.
            if matches!(reg_kinds[ins.a() as usize], RegKind::Unset)
                && !RegKind::unify(&mut reg_kinds[ins.a() as usize], *ret_kind)
            {
                return None;
            }
        }
        Op::Return0 | Op::Jmp => {}
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

pub(super) fn sweep_table_sets(st: &mut KindSweep, ins: Inst) -> Option<()> {
    let KindSweep {
        reg_kinds,
        latest_writer_kind,
        maybe_table,
        is_nil_writer,
        ..
    } = st;
    match ins.op() {
        Op::NewTable => {
            // R[A] = fresh empty table.
            if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Table) {
                return None;
            }
            latest_writer_kind[ins.a() as usize] = RegKind::Table;
            // A freshly-NewTable'd register isn't a
            // maybe-Int — it's definitely a Table — so
            // clear the maybe_table tag too. arith etc.
            // already bail via the RegKind::Table check.
            maybe_table[ins.a() as usize] = false;
            is_nil_writer[ins.a() as usize] = false;
        }
        Op::SetList => {
            // `R[A][1..=B] = R[A+1..A+B]`. R[A]
            // must be Table; the per-element kinds (Int /
            // Float / Table / Nil) are inspected at emit time
            // (current_kinds + current_is_nil) so we tag-store
            // correctly. No kind constraint pushed onto
            // R[A+i] here — let upstream writers pin them.
            if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Table) {
                return None;
            }
        }
        Op::SetTable => {
            // R[A] (table) Table. Key/value pair must
            // be either (Int, Int) or (Float, Float). Mixed
            // shapes (Int key + Float value) aren't required
            // by any current bench source — luna's frontend
            // emits `Move + Move + Move + SetTable` where
            // the Moves come from the same loop var (so the
            // pair shares kind). Bail mixed shapes.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let c = ins.c() as usize;
            if !RegKind::unify(&mut reg_kinds[a], RegKind::Table) {
                return None;
            }
            // `t[nil] = x` raises in interp (
            // "table index is nil"); the JIT's Int helper
            // would silently set `t[0] = x`. `t[k] = nil`
            // would write `Int(0)` instead of removing the
            // entry. Either Nil operand bails to interp.
            if is_nil_writer[b] || is_nil_writer[c] {
                return None;
            }
            // Key and value must unify with each other —
            // they're typically two Moves of the same source.
            let kb = reg_kinds[b];
            let kc = reg_kinds[c];
            if !RegKind::unify(&mut reg_kinds[b], kc) {
                return None;
            }
            if !RegKind::unify(&mut reg_kinds[c], kb) {
                return None;
            }
            // Pin them to Int by default if still Unset; the
            // Float branch is reachable only when one side
            // was already Float-pinned by a prior op (e.g. a
            // LoadF or a Float-typed loop var).
            let resolved = reg_kinds[b];
            if matches!(resolved, RegKind::Unset) {
                if !RegKind::unify(&mut reg_kinds[b], RegKind::Int) {
                    return None;
                }
                if !RegKind::unify(&mut reg_kinds[c], RegKind::Int) {
                    return None;
                }
            } else if !matches!(resolved, RegKind::Int | RegKind::Float) {
                // Table-typed key/value — out of scope.
                return None;
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

pub(super) fn sweep_table_gets(st: &mut KindSweep, c: ChunkIn<'_>, ins: Inst) -> Option<()> {
    let ChunkIn { float_only, .. } = c;
    let KindSweep {
        reg_kinds,
        latest_writer_kind,
        maybe_table,
        is_nil_writer,
        ..
    } = st;
    match ins.op() {
        Op::GetI => {
            // R[A] = R[B][imm(C)]. R[B] must be Table;
            // R[A] is Int (matches the static Int-only store
            // expectation of `luna_jit_table_get_int`).
            // the helper returns raw payload
            // bits regardless of the slot's actual Value
            // tag; if the table stored a Table at that
            // index the read value is a Gc<Table> pun.
            // Mark R[A] maybe_table so subsequent arith /
            // Lt-Le / ForPrep bail conservatively.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            if !RegKind::unify(&mut reg_kinds[b], RegKind::Table) {
                return None;
            }
            if !RegKind::unify(&mut reg_kinds[a], RegKind::Int) {
                return None;
            }
            maybe_table[a] = true;
            is_nil_writer[a] = false;
        }
        Op::GetTable => {
            // R[A] = R[B][R[C]]. R[B] is Table.
            // R[C] is a key — Int or Float are both fine
            // (helper handles Float keys via `Table::get`,
            // which normalises integral Floats back to the
            // Int slot). A Table-typed key would be a
            // semantics-level error PUC raises ("attempt to
            // index with a table value" downstream); we bail
            // the JIT path. R[A] is NOT forced to Int —
            // 5.1/5.2 frontends often emit `LoadF R[C]=1.0`
            // and then `GetTable R[A] = R[B][R[C]]` reusing
            // R[A]=R[C]'s slot; forcing Int would conflict
            // with the Float pin. The Variable stays Float
            // and the emit bitcasts the i64 helper return
            // back to F64 (`aligned_def`); a downstream
            // Return1 / arith bitcasts F64→I64 to recover
            // the raw payload bits.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let c = ins.c() as usize;
            if !RegKind::unify(&mut reg_kinds[b], RegKind::Table) {
                return None;
            }
            if matches!(reg_kinds[c], RegKind::Table)
                || matches!(latest_writer_kind[c], RegKind::Table)
                || maybe_table[c]
            {
                return None;
            }
            // Nil key would call `Table::get(Nil)`
            // which is well-defined (returns Nil) but the
            // raw-payload contract breaks: 0 bits for Nil
            // can't be distinguished from a valid `Int(0)`
            // stored at that slot. Bail to interp.
            if is_nil_writer[c] {
                return None;
            }
            // Default-kind for GetTable destination depends on the
            // dialect — luna's storage helper returns raw payload
            // bits regardless of the slot's actual atag, and the
            // method-JIT writeback uses reg_kinds[a] to pick the
            // Value tag back. Under 5.1/5.2 (`float_only`) numbers
            // are ALWAYS Float — `{10, 20, 30}` stores Float bits
            // at each slot, so `t[i]` defaults to Float result.
            // Under 5.3+ integer literals stay Int — default Int.
            //
            // NOTE: `pre53` (= version ≤ 5.3) is INCORRECT here
            // — it includes 5.3 which has the integer subtype.
            // Use `float_only` (= version ≤ 5.2) to gate the
            // Float default. See the 5.3 test
            // `tests/it/jit_dialect_audit.rs::audit_gettable_computed_key`.
            let default_kind = if float_only {
                RegKind::Float
            } else {
                RegKind::Int
            };
            if matches!(reg_kinds[a], RegKind::Unset) {
                reg_kinds[a] = default_kind;
            }
            latest_writer_kind[a] = default_kind;
            maybe_table[a] = true;
            is_nil_writer[a] = false;
        }
        Op::Len => {
            // R[A] = #R[B]. R[B] Table; R[A] holds the
            // Int length helper return.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            if !RegKind::unify(&mut reg_kinds[b], RegKind::Table) {
                return None;
            }
            // Len's i64 helper return goes through
            // `aligned_def`'s bitcast on the writer side, so
            // the slot's declared type need not be Int. A
            // Float-pinned slot (5.1/5.2 reuse the ForPrep
            // init slot for `#t` after the loop) is fine —
            // the F64 Variable holds the i64 bits reinterpret,
            // and the downstream `Return1` (whose emit bitcasts
            // F64→I64 when the slot's declared Float) recovers
            // them. Track the active write kind via
            // `latest_writer_kind` so `ret_kind` derives from
            // Len's Int, not from an earlier Float writer.
            match reg_kinds[a] {
                RegKind::Int | RegKind::Float => { /* keep declared */ }
                RegKind::Unset => {
                    reg_kinds[a] = RegKind::Int;
                }
                RegKind::Table => return None,
            }
            latest_writer_kind[a] = RegKind::Int;
            // `Len`'s result is always a real Int — clear
            // any prior maybe_table tag.
            maybe_table[a] = false;
            is_nil_writer[a] = false;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
