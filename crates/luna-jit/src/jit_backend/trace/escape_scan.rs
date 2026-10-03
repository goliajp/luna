use super::*;

/// forward sweep over `record.ops[..effective_end]` to
/// classify NewTable sites. Conservative — over-escape is OK; the
/// rules below are correctness-preserving for any future emit (a
/// Sinkable site can always be heap-allocated; an Escaped site
/// MUST be).
///
/// Sweep rules:
/// - `Op::NewTable A=a B=cap`: always a new `Sinkable` site bound at
///   `(depth, a)` with `array_cap = B` (0 for `B == 0`, whose site
///   can only sink hash writes through `SetField`).
/// - `Op::SetList A=a`: the count is B, or the recorded `var_count`
///   when `B == 0`. Through a bound site with `C == 0`, no `k` and
///   `array_cap` equal to that count → sunk array init; otherwise the
///   site escapes. Bound source slots `A+1..=A+count` escape (nested
///   sinks are not handled).
/// - `Op::SetI A B_imm C`: a bound value slot C escapes. Through a
///   bound target, a key in `1..=array_cap` → sunk write; any other
///   key escapes the target.
/// - `Op::SetTable`: as `SetI`, with the key folded from a `LoadI`
///   (through `Move`s) by `const_fold_int_key`; no folded key → the
///   target escapes.
/// - `Op::SetField`: a bound value slot escapes; through a bound
///   target the constant key gets a hash slot → sunk write.
/// - `Op::GetI`: bound B with a key in `1..=array_cap` → sunk read,
///   else B escapes. `Op::GetField`: bound B with a key some earlier
///   `SetField` gave a slot → sunk read, else B escapes.
///   `Op::GetTable` / `Op::Len`: bound B escapes. All unbind A.
/// - `Op::Move A=dst B=src`: A becomes an alias of src's site (or
///   unbound); src stays bound and does not escape.
/// - `Op::Call A=fn B=narg+1`: bound argument slots `A+1..A+B-1` and a
///   bound A escape; A is unbound afterwards.
/// - `Op::Return1 A=a`: a bound A escapes. `Op::Return0`: nothing.
/// - Cmp ops (`Op::Lt/Le/Eq/EqK`): nothing escapes; the side-exit
///   emit materializes live sites at depth 0, and pre-emit demotes
///   sites when a cmp sits at depth > 0.
/// - `Op::LoadNil`: unbinds `A..=A+B`. `Op::Close`: every live
///   binding escapes.
/// - Other writer ops (arith / loads / GetUpval / GetTabUp / Concat /
///   Closure / etc.): unbind A.
///
/// Terminator handling (the op at `effective_end`, if any):
/// - `TraceEnd::Call`: every live binding escapes (the interpreter
///   resumes at the call with the whole frame live).
/// - `TraceEnd::ForLoop`: nothing escapes; the loop exit resumes
///   outside the body, where its locals are dead.
/// - `TraceEnd::InlineAbort` / `SelfLink` / `DownRec`: every live
///   binding escapes.
/// - `TraceEnd::Return`: `Return1` only → R[A] escapes; `Return0` is
///   a no-op.
pub(super) fn escape_analyze(
    record: &TraceRecord,
    effective_end: usize,
    end_kind: Option<TraceEnd>,
    head_proto: Gc<Proto>,
) -> EscapeAnalysis {
    let max_stack = head_proto.max_stack as usize;
    // one bindings row per inline depth the record reaches
    let max_depth = record
        .ops
        .iter()
        .map(|r| r.inline_depth as usize)
        .max()
        .unwrap_or(0)
        .min(MAX_INLINE_DEPTH as usize)
        + 1;
    if max_stack == 0 {
        let upper0 = effective_end.min(record.ops.len());
        return EscapeAnalysis {
            sites: Vec::new(),
            op_actions: vec![None; upper0],
            live_at_op: vec![Vec::new(); upper0],
            accum_sites: Vec::new(),
            accum_live_at_op: Vec::new(),
        };
    }

    let mut bindings: Vec<Vec<Option<usize>>> = vec![vec![None; max_stack]; max_depth];
    let mut sites: Vec<AllocSite> = Vec::new();
    let upper = effective_end.min(record.ops.len());
    let mut op_actions: Vec<Option<OpAction>> = vec![None; upper];
    let mut live_at_op: Vec<Vec<u32>> = vec![Vec::new(); upper];

    for i in 0..upper {
        let cur_depth = record.ops[i].inline_depth as usize;
        // clear stale bindings from popped deeper
        // inline frames. After a Return*, control transitions
        // from depth N+1 back to depth N (the next op is at depth
        // N). The bindings rows for depths > N hold sites that
        // were tracked inside the now-popped frame; their
        // registers no longer point to live data. Without this
        // clear, live_at_op snapshots would include stale sites
        // and emit_materialize would index wrong inline windows.
        // (nothing is bound before the first site)
        if !sites.is_empty() {
            for d in (cur_depth + 1)..bindings.len() {
                for slot in bindings[d].iter_mut() {
                    *slot = None;
                }
            }
            // snapshot live sunk bindings BEFORE the op
            // processes (each cmp emit uses live_at_op[cmp_idx] to
            // materialise the right virt slots).
            let mut live_snap: Vec<u32> = Vec::new();
            for row in bindings.iter() {
                for &slot in row.iter() {
                    if let Some(sid) = slot {
                        live_snap.push(sid as u32);
                    }
                }
            }
            live_at_op[i] = live_snap;
        }

        let rop = &record.ops[i];
        let depth = rop.inline_depth;
        let ins = rop.inst;
        let a = ins.a();
        let op = ins.op();

        if (depth as usize) >= max_depth || (a as usize) >= max_stack {
            // The lowerer bails on OOB; skip silently here so the
            // sweep stays a side-effect-free analysis.
            continue;
        }
        sweep_op(
            record,
            i,
            rop,
            depth,
            ins,
            a,
            op,
            max_stack,
            &mut bindings,
            &mut sites,
            &mut op_actions,
        );
    }

    escape_at_end(
        record,
        effective_end,
        end_kind,
        max_stack,
        max_depth,
        &bindings,
        &mut sites,
    );

    let accum_sites = detect_accumulators(record, effective_end, head_proto);
    let accum_live_at_op = vec![Vec::new(); op_actions.len()];
    EscapeAnalysis {
        sites,
        op_actions,
        live_at_op,
        accum_sites,
        accum_live_at_op,
    }
}

/// One op of the body sweep in `escape_analyze`.
fn sweep_op(
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
            // way, if B (source table) is bound, escape.
            let b = ins.b();
            if (b as usize) < max_stack
                && let Some(sid) = lookup(bindings, depth, b)
            {
                mark_escape(sites, sid);
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
            let b = ins.b();
            if b > 0 {
                for off in 1..b {
                    let src = a.wrapping_add(off);
                    if (src as usize) < max_stack
                        && let Some(src_sid) = lookup(bindings, depth, src)
                    {
                        mark_escape(sites, src_sid);
                    }
                }
            }
            if let Some(fn_sid) = lookup(bindings, depth, a) {
                mark_escape(sites, fn_sid);
            }
            unbind(bindings, depth, a);
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
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::IDiv | Op::Mod
        | Op::Pow | Op::BAnd | Op::BOr | Op::BXor | Op::Shl | Op::Shr
        | Op::Unm | Op::BNot
        | Op::LoadI | Op::LoadF | Op::LoadK
        | Op::GetUpval | Op::GetTabUp | Op::Concat
        // Op::Closure writes a fresh LuaClosure
        // pointer into R[A]; no NewTable site lives there.
        // Op::GetField has its own arm above
        // (escapes R[B] receiver); not in this catch-all.
        | Op::Closure => {
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
        Op::Close => {
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
        Op::Jmp | Op::ForLoop | Op::Return => {}
        _ => {
            unbind(bindings, depth, a);
        }
    }
}
