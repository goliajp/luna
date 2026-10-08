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

/// An op without a sunk path. A sunk table lives only in its virtual
/// slots, so the bits in a register bound to it are stale: an op that
/// reads such a register sees the wrong value, and the table has to be
/// a real one. The registers the op writes no longer hold the table.
pub(super) fn sweep_plain_op(
    rop: &RecordedOp,
    depth: u8,
    max_stack: usize,
    bindings: &mut [Vec<Option<usize>>],
    sites: &mut [AllocSite],
) {
    let ins = rop.inst;
    let a = ins.a();
    let (reads, writes) = super::slots::rw_ranges(ins);
    let mut read = |r: u32| {
        if (r as usize) < max_stack
            && let Some(sid) = lookup(bindings, depth, r)
        {
            mark_escape(sites, sid);
        }
    };
    for (lo, n) in reads {
        (lo..lo + n).for_each(&mut read);
    }
    match ins.op() {
        // a closure captures the registers of its in-stack upvalues
        Op::Closure => {
            if let Some(p) = rop.proto.protos.get(ins.bx() as usize) {
                p.upvals
                    .iter()
                    .filter(|d| d.in_stack)
                    .for_each(|d| read(u32::from(d.index)));
            }
        }
        // the ipairs path reads the previous value
        op if op.is_tfor_call() => read(a + op.for_layout().map_or(0, |l| l.var()) + 1),
        // a multi-value Return of an inlined callee hands R[A..] up
        Op::Return if ins.b() == 0 => (a..max_stack as u32).for_each(&mut read),
        _ => {}
    }
    for (lo, n) in writes {
        (lo..lo + n).for_each(|r| unbind(bindings, depth, r));
    }
    match ins.op() {
        // the ipairs path writes the control variable as well
        op if op.is_tfor_call() => unbind(
            bindings,
            depth,
            a + op.for_layout().map_or(0, |l| l.control()),
        ),
        // a variable count of values from R[A] up
        Op::Vararg | Op::GetVarg => (a..max_stack as u32).for_each(|r| unbind(bindings, depth, r)),
        _ => {}
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
            let sizes = luna_core::runtime::table::new_table_sizes(ins.b(), ins.c(), ins.k());
            unbind(bindings, depth, a);
            let sid = sites.len();
            sites.push(AllocSite {
                op_idx: i,
                pc: rop.pc,
                a,
                inline_depth: depth,
                array_cap: sizes.map_or(0, |(asize, _)| asize as u32),
                table_ops: crate::jit_backend::pack_table_ops(ins),
                hash_keys: Vec::new(),
                // sizes past a table's limit: the helper raises the error
                state: if sizes.is_some() {
                    EscapeState::Sinkable
                } else {
                    EscapeState::Escaped
                },
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
            // a bound key register is read as a value too
            for r in [ins.b(), ins.c()] {
                if (r as usize) < max_stack
                    && let Some(src_sid) = lookup(bindings, depth, r)
                {
                    mark_escape(sites, src_sid);
                }
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
                // The terminator either leaves the loop, where the
                // registers below the loop's own (`A`) are the
                // enclosing scope's locals, or jumps back to the
                // body's first op, where the next iteration reads
                // them and the control registers `A..A+4`. A sunk
                // table never reaches either: its registers hold
                // stale bits, and the back-edge has no materialise
                // path. So a table still bound there (`last = t`,
                // `prev = {n = i}`) is a real one. Registers from
                // `A + 4` up are the body's locals, out of scope at
                // both places, and keep their tables sunk.
                let row = depth as usize;
                if row < bindings.len() {
                    let limit = (a as usize).saturating_add(4).min(bindings[row].len());
                    for &slot in &bindings[row][..limit] {
                        if let Some(sid) = slot {
                            mark_escape(sites, sid);
                        }
                    }
                }
            }
            TraceEnd::Return => {
                // the returned values: R[A] for Return1, R[A..A+B-1)
                // for Return (B = 0: up to the top)
                let end = match op {
                    Op::Return1 => a.saturating_add(1),
                    Op::Return if term.inst.b() == 0 => max_stack as u32,
                    Op::Return => a.saturating_add(term.inst.b().saturating_sub(1)),
                    _ => a,
                };
                if in_range {
                    for r in a..end.min(max_stack as u32) {
                        if let Some(sid) = lookup(bindings, depth, r) {
                            mark_escape(sites, sid);
                        }
                    }
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
    } else if end_kind.is_none() {
        // No terminator: the trace runs back to its head, either
        // through the dispatcher or along its own back-edge. Which
        // registers are still in scope there is not known (a while
        // loop's locals have no `A` to split them by), so every table
        // still bound is a real one.
        escape_all_live(bindings, sites);
    }
}
