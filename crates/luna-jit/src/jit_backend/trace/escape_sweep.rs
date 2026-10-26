use super::*;

mod sites;
use sites::{hash_slot, new_table_site};

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
        Op::NewTable => new_table_site(i, rop, depth, ins, a, bindings, sites, op_actions),
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
                let (slot, grow_to) = hash_slot(sites, sid, key_const_idx);
                op_actions[i] = Some(OpAction::SetFieldSunkWrite {
                    site_idx: sid as u32,
                    hash_slot: slot as u32,
                    grow_to,
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
