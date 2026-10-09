//! One op of the escape sweep (see [`super::escape_scan`]).

use super::*;

/// One op of the body sweep in `escape_analyze`.
pub(super) fn sweep_op(
    record: &TraceRecord,
    i: usize,
    rop: &RecordedOp,
    depth: u8,
    ins: Inst,
    a: u32,
    op: Op,
    max_stack: usize,
    bindings: &mut [Vec<Option<usize>>],
    sites: &mut Vec<AllocSite>,
    op_actions: &mut [Option<OpAction>],
) {
    match op {
        Op::NewTable | Op::SetList | Op::SetI | Op::SetField | Op::SetTable => sweep_table_write(
            record, i, rop, depth, ins, a, op, max_stack, bindings, sites, op_actions,
        ),
        Op::GetField => {
            // `R[A] := R[B][K[C]:string]`. If R[B]
            // is bound to a Sinkable site AND the key has been
            // seen on this site (i.e. exists in site.hash_keys),
            // tag sunk read. Unknown key (first GetField for it
            // without a prior SetField) escapes the site —
            // reading uninitialised hash slot would be Nil at
            // runtime, but the trace's virt slot would hold
            // undefined value. Conservative escape.
            let b_reg = ins.b();
            let key_const_idx = ins.c();
            if (b_reg as usize) < max_stack
                && let Some(sid) = lookup(bindings, depth, b_reg)
            {
                let slot_opt = sites[sid]
                    .hash_keys
                    .iter()
                    .position(|&k| k == key_const_idx);
                match slot_opt {
                    Some(s) => {
                        op_actions[i] = Some(OpAction::GetFieldSunkRead {
                            site_idx: sid as u32,
                            hash_slot: s as u32,
                        });
                    }
                    None => {
                        mark_escape(sites, sid);
                    }
                }
            }
            unbind(bindings, depth, a);
        }
        Op::GetI => {
            // R[A] := R[B][C_imm]. Sunk-emit support: B must be
            // bound to a Sinkable site AND the immediate key C
            // must fall in `1..=array_cap`. Anything else: site
            // (if any) escapes.
            let b = ins.b();
            let c = ins.c();
            if (b as usize) < max_stack
                && let Some(sid) = lookup(bindings, depth, b)
            {
                if c >= 1 && c <= sites[sid].array_cap {
                    op_actions[i] = Some(OpAction::GetIRead {
                        site_idx: sid as u32,
                        key: c,
                    });
                } else {
                    // OOB or zero key — sunk emit can't represent
                    // this read; force heap path.
                    mark_escape(sites, sid);
                }
            }
            unbind(bindings, depth, a);
        }
        Op::GetTable | Op::Len => {
            // GetTable: dynamic key — can't constant-fold. Len:
            // we know cap, but Len has no sunk emit. Either
            // way, if B (source table) is bound, escape. So does a
            // bound key register of GetTable: it is read as a value.
            let b = ins.b();
            let keys: &[u32] = if op == Op::GetTable {
                &[b, ins.c()]
            } else {
                &[b]
            };
            for &r in keys {
                if (r as usize) < max_stack
                    && let Some(sid) = lookup(bindings, depth, r)
                {
                    mark_escape(sites, sid);
                }
            }
            unbind(bindings, depth, a);
        }
        Op::Move => {
            // Move is a binding alias: the dst reg
            // now references the same sunk site as src; src's
            // own binding stays. No escape — both regs are
            // interior-trace aliases, and downstream ops drive
            // escape via their own rules (SetI/SetTable/Len/Call
            // arg/Return1). Escaping src here would collapse any
            // sunk site touched by Lua 5.5 frontend's `Move
            // temp=R[t]; SetI temp[k]=v` lowering of `t[k]=v`.
            let b = ins.b();
            let src_sid = if (b as usize) < max_stack {
                lookup(bindings, depth, b)
            } else {
                None
            };
            unbind(bindings, depth, a);
            if let Some(sid) = src_sid {
                bindings[depth as usize][a as usize] = Some(sid);
            }
        }
        Op::Call => {
            // `B = 0` passes everything up to the top as arguments, and
            // `C = 0` leaves the results up to the top: neither range has
            // a fixed end, so take every register from A up
            let b = ins.b();
            let c = ins.c();
            let args_end = if b == 0 {
                max_stack as u32
            } else {
                a.saturating_add(b)
            };
            for src in a..args_end.min(max_stack as u32) {
                if let Some(src_sid) = lookup(bindings, depth, src) {
                    mark_escape(sites, src_sid);
                }
            }
            let results_end = if c == 0 {
                max_stack as u32
            } else {
                a.saturating_add(c.saturating_sub(1)).max(a + 1)
            };
            for r in a..results_end.min(max_stack as u32) {
                unbind(bindings, depth, r);
            }
        }
        Op::Return1 => {
            if let Some(sid) = lookup(bindings, depth, a) {
                mark_escape(sites, sid);
            }
        }
        Op::Return0 => {}
        Op::Lt | Op::Le | Op::Eq | Op::EqK => {
            // cmp side-exits no longer auto-escape
            // live sunk sites. `emit_materialize_*` runs
            // at every cmp side-exit emit point to materialize
            // the live virt slots into a heap `Gc<Table>` +
            // override the per-exit-tags entry to `Table`.
            //
            // depth=0 cmps materialize at the side-exit emit
            // path (Op::Lt/Le/Eq/EqK arm `else` branch — the
            // non-inline path). depth>0 cmps would need the
            // same machinery in the `per_exit_inline` arm; the
            // site is demoted to Escaped instead (see the
            // `has_inline_cmp` gate in pre-emit demote).
            //
            // A compared table is compared by address (and its
            // metatable is read), which a sunk table does not have.
            let operands: &[u32] = if op == Op::EqK { &[a] } else { &[a, ins.b()] };
            for &r in operands {
                if (r as usize) < max_stack
                    && let Some(sid) = lookup(bindings, depth, r)
                {
                    mark_escape(sites, sid);
                }
            }
        }
        Op::LoadI | Op::LoadF | Op::LoadK | Op::GetUpval | Op::GetTabUp => {
            unbind(bindings, depth, a);
        }
        Op::LoadNil => {
            // writes Nil to R[A..=A+B]. Each dest
            // slot loses its binding (a sunk site whose A is
            // overwritten with Nil is no longer reachable via
            // that slot — the actual table pointer is gone).
            let b = ins.b();
            for k in 0..=b {
                let r = a.wrapping_add(k);
                if (r as usize) < max_stack {
                    unbind(bindings, depth, r);
                }
            }
        }
        Op::Close | Op::JmpClose | Op::JmpCloseBack => {
            // Op::Close A closes open upvals at slot
            // ≥ A. Open upvals point at vm.stack — the trace
            // can't keep them only in virt slots, so any live
            // sunk site whose slot ≥ A must escape (helper's
            // close_from reads vm.stack[s] to seal each upval).
            // Conservative: escape ALL live bindings (the helper
            // path also spills all live regs via emit, so this
            // matches the IR contract).
            escape_all_live(bindings, sites);
        }
        // every other op reads and writes registers through the plain
        // path (arithmetic, Concat, Closure captures, SetUpval,
        // SetTabUp, Test, TestSet, Not, Self, TForCall, multi-value
        // Return, ...)
        _ => sweep_plain_op(rop, depth, max_stack, bindings, sites),
    }
}
