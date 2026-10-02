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
    let max_depth = (MAX_INLINE_DEPTH as usize) + 1;
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

fn mark_escape(sites: &mut [AllocSite], sid: usize) {
    if sites[sid].state == EscapeState::Sinkable {
        sites[sid].state = EscapeState::Escaped;
    }
}
fn lookup(bindings: &[Vec<Option<usize>>], depth: u8, reg: u32) -> Option<usize> {
    let d = depth as usize;
    let r = reg as usize;
    if d < bindings.len() && r < bindings[d].len() {
        bindings[d][r]
    } else {
        None
    }
}
fn unbind(bindings: &mut [Vec<Option<usize>>], depth: u8, reg: u32) {
    let d = depth as usize;
    let r = reg as usize;
    if d < bindings.len() && r < bindings[d].len() {
        bindings[d][r] = None;
    }
}
fn escape_all_live(bindings: &[Vec<Option<usize>>], sites: &mut [AllocSite]) {
    for row in bindings.iter() {
        for &slot in row.iter() {
            if let Some(sid) = slot {
                mark_escape(sites, sid);
            }
        }
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

/// The table-building ops of `sweep_op`.
fn sweep_table_write(
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
        Op::NewTable => {
            // admit all NewTable shapes as
            // potential sites. For hash-only (b == 0) the
            // array_cap is 0 and the site only sunk-emits via
            // SetField/GetField. For mixed (b > 0, c > 0) the
            // array part sunk-emits; hash slots accumulate via
            // SetField scan.
            let cap = ins.b();
            let _c_hash = ins.c();
            unbind(bindings, depth, a);
            let sid = sites.len();
            sites.push(AllocSite {
                op_idx: i,
                pc: rop.pc,
                a,
                inline_depth: depth,
                array_cap: cap,
                hash_keys: Vec::new(),
                state: EscapeState::Sinkable,
            });
            bindings[depth as usize][a as usize] = Some(sid);
            // tag this op so emit can take the sunk
            // path (no NewTable helper call).
            op_actions[i] = Some(OpAction::NewTableSite {
                site_idx: sid as u32,
            });
        }
        Op::SetList => {
            // B=0 form: use the recorder's var_count
            // snapshot (= top - A - 1 at the SetList op) as the
            // effective B. Otherwise the bytecode's B is the count.
            let b_bytecode = ins.b();
            let c = ins.c();
            let effective_b = if b_bytecode == 0 {
                record.ops[i].var_count.unwrap_or_default()
            } else {
                b_bytecode
            };
            if let Some(sid) = lookup(bindings, depth, a) {
                if c != 0 || ins.k() || sites[sid].array_cap != effective_b {
                    mark_escape(sites, sid);
                } else {
                    // supported form; tag for
                    // sunk emit (def_var virt slots from source
                    // registers). For B=0, the source range size
                    // is the recorded var_count.
                    op_actions[i] = Some(OpAction::SetListWrite {
                        site_idx: sid as u32,
                    });
                }
                for off in 1..=effective_b {
                    let src = a.wrapping_add(off);
                    if (src as usize) < max_stack
                        && let Some(src_sid) = lookup(bindings, depth, src)
                    {
                        mark_escape(sites, src_sid);
                    }
                }
            }
        }
        Op::SetI => {
            // `R[A][B_imm] := R[C]`. Target slot
            // bound to a Sinkable site + key in 1..=cap →
            // tag SetISunkWrite (emit `def_var`s the value into
            // the matching virt slot). Otherwise the target
            // site escapes (helper-path SetI writes through the
            // real heap table). The value source slot escapes
            // unconditionally if it's bound — sinking a sunk
            // table into another sunk table's slot would need
            // pointer-aliasing the virt slot, which is unsupported.
            let c = ins.c();
            if (c as usize) < max_stack
                && let Some(src_sid) = lookup(bindings, depth, c)
            {
                mark_escape(sites, src_sid);
            }
            if let Some(sid) = lookup(bindings, depth, a) {
                let key = ins.b();
                if key >= 1 && key <= sites[sid].array_cap {
                    op_actions[i] = Some(OpAction::SetISunkWrite {
                        site_idx: sid as u32,
                        key,
                    });
                } else {
                    // OOB key — sunk emit can't represent this
                    // write; helper path needed → site escapes.
                    mark_escape(sites, sid);
                }
            }
        }
        Op::SetField => {
            // `R[A][K[B]:string] := R[C]`. If R[A]
            // is bound to a Sinkable site AND R[C]'s value is not
            // itself a bound site (sinking a site into another
            // site's hash slot is out of scope), tag sunk: push
            // the key's const idx to site.hash_keys (if not
            // already there) and record the slot in OpAction.
            // Otherwise the target site escapes.
            let c = ins.c();
            let key_const_idx = ins.b();
            if (c as usize) < max_stack
                && let Some(src_sid) = lookup(bindings, depth, c)
            {
                mark_escape(sites, src_sid);
            }
            if let Some(sid) = lookup(bindings, depth, a) {
                // Find or insert the hash slot for this key.
                let slot_opt = sites[sid]
                    .hash_keys
                    .iter()
                    .position(|&k| k == key_const_idx);
                let slot = match slot_opt {
                    Some(s) => s,
                    None => {
                        let s = sites[sid].hash_keys.len();
                        sites[sid].hash_keys.push(key_const_idx);
                        s
                    }
                };
                op_actions[i] = Some(OpAction::SetFieldSunkWrite {
                    site_idx: sid as u32,
                    hash_slot: slot as u32,
                });
            }
        }
        Op::SetTable => {
            // `R[A][R[B]] := R[C]`. Sunk emit requires
            // a compile-time-known int key. `const_fold_int_key`
            // walks back from this op looking for a LoadI (via a
            // Move chain) that pinned R[B]'s value to a literal
            // in 1..=cap. If found, tag SetTableSunkWrite (same
            // emit as SetI sunk path). Otherwise the target site
            // escapes (helper path runs through real heap table).
            // Value source still escapes if bound (same as SetI).
            let c = ins.c();
            if (c as usize) < max_stack
                && let Some(src_sid) = lookup(bindings, depth, c)
            {
                mark_escape(sites, src_sid);
            }
            if let Some(sid) = lookup(bindings, depth, a) {
                let cap = sites[sid].array_cap;
                let key_reg = ins.b();
                if let Some(key) = const_fold_int_key(record, i, key_reg, cap) {
                    op_actions[i] = Some(OpAction::SetTableSunkWrite {
                        site_idx: sid as u32,
                        key,
                    });
                } else {
                    mark_escape(sites, sid);
                }
            }
        }
        _ => unreachable!("sweep_op routes only table writes here"),
    }
}

/// What the trace terminator (the op at `effective_end`) forces to escape.
fn escape_at_end(
    record: &TraceRecord,
    effective_end: usize,
    end_kind: Option<TraceEnd>,
    max_stack: usize,
    max_depth: usize,
    bindings: &[Vec<Option<usize>>],
    sites: &mut [AllocSite],
) {
    if let Some(end) = end_kind
        && effective_end < record.ops.len()
    {
        let term = &record.ops[effective_end];
        let depth = term.inline_depth;
        let a = term.inst.a();
        let op = term.inst.op();
        let in_range = (depth as usize) < max_depth && (a as usize) < max_stack;
        match end {
            // The interpreter resumes at the call with the whole frame
            // live: a table still under construction (`{f()}`, whose
            // SetList follows the call) or any other sunk table read
            // after it must be a real one.
            TraceEnd::Call | TraceEnd::InlineAbort => {
                escape_all_live(bindings, sites);
            }
            TraceEnd::ForLoop => {
                // DO NOT auto-escape on a ForLoop
                // terminator. ForLoop's IR side-exit fires on
                // loop exit; interp resumes OUTSIDE the loop,
                // where any `local t = {...}` declared inside
                // the body is out of scope (parser frees the
                // register slot at loop end). The dispatcher's
                // exit-tag override (Sinkable slot → Untouched)
                // keeps the slot reading as its entry tag.
                //
                // A mid-body cmp side-exit would still need
                // materialise (interp resumes IN the loop body
                // at the side-exit PC, where `t` may still be
                // accessed) — the cmp arm in the body sweep
                // already escapes live bindings, and the
                // pre-emit `body_has_cmp` gate is a defensive
                // backstop.
            }
            TraceEnd::Return => {
                if matches!(op, Op::Return1)
                    && in_range
                    && let Some(sid) = lookup(bindings, depth, a)
                {
                    mark_escape(sites, sid);
                }
            }
            TraceEnd::SelfLink(_) => {
                // self-link close. Body loops with
                // snapshot-restore (deepest-frame's window →
                // head-frame's window). Every live binding at
                // close must be marshalled back across the
                // back-edge so the next iter sees a coherent
                // window, same as InlineAbort's blanket escape.
                escape_all_live(bindings, sites);
            }
            TraceEnd::DownRec { .. } => {
                // down-rec close. Every exit (the safe deopt
                // tail and the retf-guard exit alike) returns
                // through the caller window, so every live
                // binding at close must be marshalled back into
                // it — same blanket-escape posture as SelfLink /
                // InlineAbort.
                escape_all_live(bindings, sites);
            }
        }
    }
}
