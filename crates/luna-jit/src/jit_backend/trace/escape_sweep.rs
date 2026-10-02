use super::*;

pub(super) fn mark_escape(sites: &mut [AllocSite], sid: usize) {
    if sites[sid].state == EscapeState::Sinkable {
        sites[sid].state = EscapeState::Escaped;
    }
}

pub(super) fn lookup(bindings: &[Vec<Option<usize>>], depth: u8, reg: u32) -> Option<usize> {
    let d = depth as usize;
    let r = reg as usize;
    if d < bindings.len() && r < bindings[d].len() {
        bindings[d][r]
    } else {
        None
    }
}

pub(super) fn unbind(bindings: &mut [Vec<Option<usize>>], depth: u8, reg: u32) {
    let d = depth as usize;
    let r = reg as usize;
    if d < bindings.len() && r < bindings[d].len() {
        bindings[d][r] = None;
    }
}

pub(super) fn escape_all_live(bindings: &[Vec<Option<usize>>], sites: &mut [AllocSite]) {
    for row in bindings.iter() {
        for &slot in row.iter() {
            if let Some(sid) = slot {
                mark_escape(sites, sid);
            }
        }
    }
}

/// The table-building ops of `sweep_op`.
pub(super) fn sweep_table_write(
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
pub(super) fn escape_at_end(
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
