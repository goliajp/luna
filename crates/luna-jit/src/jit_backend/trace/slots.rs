//! Which register slots a recorded op reads and writes.

use super::*;

/// per-op (reads, writes) slot analysis. Returns the
/// slot indices an op READS from and WRITES to in the caller's
/// register window. Conservative for unknown / not-yet-classified
/// ops: read range is widened (assume reads everything in the
/// range we're aware of), writes is empty — so the safety check
/// (`child.live_in ∩ parent.body_writes`) errs on the side of
/// bailing the side trace compile.
///
/// Caller is responsible for applying `inline_depth` offsets if
/// the op lives in a depth>0 inlined frame.
pub fn op_reads_writes(inst: luna_core::vm::isa::Inst) -> (Vec<u32>, Vec<u32>) {
    use luna_core::vm::isa::Op;
    let a = inst.a();
    let b = inst.b();
    let c = inst.c();
    let k = inst.k();
    match inst.op() {
        Op::Move => (vec![b], vec![a]),
        Op::LoadI | Op::LoadF | Op::LoadK | Op::LoadKx => (vec![], vec![a]),
        Op::LoadFalse | Op::LoadTrue | Op::LFalseSkip => (vec![], vec![a]),
        Op::LoadNil => {
            // R[A..=A+B] := nil
            let mut w = Vec::with_capacity((b + 1) as usize);
            for i in 0..=b {
                w.push(a + i);
            }
            (vec![], w)
        }
        Op::GetUpval => (vec![], vec![a]),
        Op::SetUpval => (vec![a], vec![]),
        Op::GetTabUp => (vec![], vec![a]),
        Op::GetTable => (vec![b, c], vec![a]),
        Op::GetI => (vec![b], vec![a]),
        Op::GetField => (vec![b], vec![a]),
        Op::SetTabUp => {
            // upval[b][const_b_or_R[B]] = R[C] / K[C]
            let mut r = Vec::new();
            if !k {
                r.push(c);
            }
            (r, vec![])
        }
        Op::SetTable => {
            let mut r = vec![a, b];
            if !k {
                r.push(c);
            }
            (r, vec![])
        }
        Op::SetI => {
            let mut r = vec![a];
            if !k {
                r.push(c);
            }
            (r, vec![])
        }
        Op::SetField => {
            let mut r = vec![a];
            if !k {
                r.push(c);
            }
            (r, vec![])
        }
        Op::NewTable => (vec![], vec![a]),
        Op::SelfOp => (vec![b], vec![a, a + 1]),
        Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Mod
        | Op::Pow
        | Op::Div
        | Op::IDiv
        | Op::BAnd
        | Op::BOr
        | Op::BXor
        | Op::Shl
        | Op::Shr => (vec![b, c], vec![a]),
        Op::Unm | Op::BNot | Op::Not | Op::Len => (vec![b], vec![a]),
        Op::Concat => {
            // R[A] := concat(R[A..A+B-1])
            let mut r = Vec::with_capacity(b as usize);
            for i in 0..b {
                r.push(a + i);
            }
            (r, vec![a])
        }
        Op::Close | Op::Tbc => (vec![], vec![]),
        Op::Jmp | Op::ExtraArg => (vec![], vec![]),
        Op::Eq | Op::Lt | Op::Le => (vec![a, b], vec![]),
        Op::EqK => (vec![a], vec![]),
        Op::Test => (vec![a], vec![]),
        Op::TestSet => (vec![b], vec![a]),
        Op::Call => {
            // R[A..A+B-1] are args (incl. fn at R[A]); writes R[A..A+C-1]
            // B=0 means variable (top); C=0 means variable. Conservative:
            // assume B,C up to a reasonable cap (use observed values).
            let nargs = if b == 0 { 0 } else { b - 1 };
            let nres = if c == 0 { 0 } else { c - 1 };
            let mut r = vec![a];
            for i in 1..=nargs {
                r.push(a + i);
            }
            let mut w = Vec::with_capacity(nres as usize);
            for i in 0..nres {
                w.push(a + i);
            }
            (r, w)
        }
        Op::TailCall => {
            let nargs = if b == 0 { 0 } else { b - 1 };
            let mut r = vec![a];
            for i in 1..=nargs {
                r.push(a + i);
            }
            (r, vec![])
        }
        Op::Return => {
            // R[A..A+B-2] returned
            let n = if b == 0 { 1 } else { b - 1 };
            let mut r = Vec::with_capacity(n as usize);
            for i in 0..n {
                r.push(a + i);
            }
            (r, vec![])
        }
        Op::Return0 => (vec![], vec![]),
        Op::Return1 => (vec![a], vec![]),
        Op::ForLoop => {
            // R[A+1] = count, R[A] = idx, R[A+2] = step, R[A+3] = ctrl
            // Reads R[A], R[A+1], R[A+2]; writes R[A], R[A+1], R[A+3].
            (vec![a, a + 1, a + 2], vec![a, a + 1, a + 3])
        }
        Op::ForPrep => {
            // Sets up the for loop: reads init/limit/step, writes idx/count/ctrl.
            (vec![a, a + 1, a + 2], vec![a, a + 1, a + 3])
        }
        Op::TForPrep => (vec![], vec![]),
        Op::TForCall => {
            // R[A+4], R[A+5], ..., R[A+3+C] := R[A](R[A+1], R[A+2])
            let mut w = Vec::with_capacity(c as usize);
            for i in 0..c {
                w.push(a + 4 + i);
            }
            (vec![a, a + 1, a + 2], w)
        }
        Op::TForLoop => {
            // If R[A+4] ~= nil: R[A+2] = R[A+4]; pc -= Bx
            (vec![a + 4], vec![a + 2])
        }
        Op::SetList => {
            // R[A] is the table; R[A+1..A+B] are values to set.
            let n = if b == 0 { 0 } else { b };
            let mut r = vec![a];
            for i in 1..=n {
                r.push(a + i);
            }
            (r, vec![])
        }
        Op::Closure => (vec![], vec![a]),
        Op::Vararg | Op::GetVarg => {
            // Writes a variable count starting at R[A]. Conservative: just write R[A].
            (vec![], vec![a])
        }
        Op::VargIdx => (vec![c], vec![a]),
        Op::ErrNNil => (vec![a], vec![]),
    }
}

/// compute the slot indices an op WRITES in the
/// caller's window, with the op's inline depth offset applied.
/// Used by `compute_body_writes` and `compute_live_in_slots`.
pub(super) fn op_writes_at_offset(rop: &RecordedOp, op_offset: u32) -> Vec<u32> {
    let (_r, w) = op_reads_writes(rop.inst);
    w.into_iter().map(|s| op_offset + s).collect()
}

fn op_reads_at_offset(rop: &RecordedOp, op_offset: u32) -> Vec<u32> {
    let (r, _w) = op_reads_writes(rop.inst);
    r.into_iter().map(|s| op_offset + s).collect()
}

/// compute the parent body's slot-write set. Walks
/// `record.ops`, applying each op's `inline_depth` offset, and
/// returns a sorted unique list of slot indices that ANY op writes.
/// Stored on `CompiledTrace.body_writes` so child side traces can
/// intersect against it at compile time.
pub fn compute_body_writes(record: &TraceRecord, op_offsets: &[u32]) -> Vec<u32> {
    let mut s: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for (i, rop) in record.ops.iter().enumerate() {
        let off = op_offsets.get(i).copied().unwrap_or(0);
        for w in op_writes_at_offset(rop, off) {
            s.insert(w);
        }
    }
    s.into_iter().collect()
}

/// compute the side trace's "live-in" slot set: slots
/// READ by some op without any prior write to the same slot within
/// `record.ops`. These are the values the side trace consumes from
/// its entry state (= what the parent wrote to reg_state at its
/// exit). If any live-in slot is ALSO in the parent's body_writes,
/// the side trace is UNSAFE to compile with internal looping (each
/// iter would re-read parent's stale write — see the s12_step_b
/// `Move R[1] = R[12]` bug).
pub fn compute_live_in_slots(record: &TraceRecord, op_offsets: &[u32]) -> Vec<u32> {
    let mut written: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut live_in: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for (i, rop) in record.ops.iter().enumerate() {
        let off = op_offsets.get(i).copied().unwrap_or(0);
        // Reads first — if a slot hasn't been written by a prior op,
        // it's live-in.
        for r in op_reads_at_offset(rop, off) {
            if !written.contains(&r) {
                live_in.insert(r);
            }
        }
        // Then mark writes (the op's writes happen "after" its reads
        // for purposes of subsequent ops).
        for w in op_writes_at_offset(rop, off) {
            written.insert(w);
        }
    }
    live_in.into_iter().collect()
}
