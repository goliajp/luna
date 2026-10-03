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
    let (reads, writes) = rw_ranges(inst);
    let runs = |v: &[(u32, u32)]| v.iter().flat_map(|&(s, n)| s..s + n).collect();
    (runs(&reads), runs(&writes))
}

/// [`op_reads_writes`] without allocating: up to three read runs and two
/// write runs, each `(first register, count)`.
pub(super) fn rw_ranges(inst: luna_core::vm::isa::Inst) -> ([(u32, u32); 3], [(u32, u32); 2]) {
    use luna_core::vm::isa::Op;
    let a = inst.a();
    let b = inst.b();
    let c = inst.c();
    let none = (0, 0);
    let one = |r: u32| (r, 1);
    let r1 = |x: u32| [one(x), none, none];
    let r2 = |x: u32, y: u32| [one(x), one(y), none];
    let r0 = [none; 3];
    let w1 = |x: u32| [one(x), none];
    let w0 = [none; 2];
    match inst.op() {
        Op::Move => (r1(b), w1(a)),
        Op::LoadI | Op::LoadF | Op::LoadK | Op::LoadKx => (r0, w1(a)),
        Op::LoadFalse | Op::LoadTrue | Op::LFalseSkip => (r0, w1(a)),
        // R[A..=A+B] := nil
        Op::LoadNil => (r0, [(a, b + 1), none]),
        Op::GetUpval => (r0, w1(a)),
        Op::SetUpval => (r1(a), w0),
        Op::GetTabUp => (r0, w1(a)),
        Op::GetTable => (r2(b, c), w1(a)),
        Op::GetI => (r1(b), w1(a)),
        Op::GetField => (r1(b), w1(a)),
        // luna's set ops always take the value from R[C] (the k flag of
        // SetField / SetTabUp marks B as a constant key)
        Op::SetTabUp => (r1(c), w0),
        Op::SetTable => ([one(a), one(b), one(c)], w0),
        Op::SetI | Op::SetField => (r2(a, c), w0),
        Op::NewTable => (r0, w1(a)),
        // a key too far for the constant field sits in R[C]
        Op::SelfOp if inst.k() => (r1(b), [(a, 2), none]),
        Op::SelfOp => (r2(b, c), [(a, 2), none]),
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
        | Op::Shr => (r2(b, c), w1(a)),
        // constant and immediate operands are not registers (a recording
        // holds these opcodes split into a load and the register form)
        Op::AddI
        | Op::SubI
        | Op::AddK
        | Op::SubK
        | Op::MulK
        | Op::ModK
        | Op::PowK
        | Op::DivK
        | Op::IDivK
        | Op::BAndK
        | Op::BOrK
        | Op::BXorK
        | Op::ShrI
        | Op::ShlI => (r1(b), w1(a)),
        Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI => (r1(a), w0),
        Op::Unm | Op::BNot | Op::Not | Op::Len => (r1(b), w1(a)),
        // R[A] := concat(R[A..A+B-1])
        Op::Concat => ([(a, b), none, none], w1(a)),
        Op::Close | Op::Tbc => (r0, w0),
        Op::Jmp | Op::ExtraArg => (r0, w0),
        Op::Eq | Op::Lt | Op::Le => (r2(a, b), w0),
        Op::EqK => (r1(a), w0),
        Op::Test => (r1(a), w0),
        Op::TestSet => (r1(b), w1(a)),
        // R[A..A+B-1] are args (incl. fn at R[A]); writes R[A..A+C-2].
        // B=0 / C=0 mean "up to top": only R[A] is counted then.
        Op::Call => (
            [(a, b.max(1)), none, none],
            [(a, c.saturating_sub(1)), none],
        ),
        Op::TailCall => ([(a, b.max(1)), none, none], w0),
        // R[A..A+B-2] returned
        Op::Return => ([(a, if b == 0 { 1 } else { b - 1 }), none, none], w0),
        Op::Return0 => (r0, w0),
        Op::Return1 => (r1(a), w0),
        // R[A+1] = count, R[A] = idx, R[A+2] = step, R[A+3] = ctrl
        // Reads R[A], R[A+1], R[A+2]; writes R[A], R[A+1], R[A+3].
        Op::ForLoop | Op::ForPrep => ([(a, 3), none, none], [(a, 2), (a + 3, 1)]),
        Op::TForPrep => (r0, w0),
        // R[A+4], R[A+5], ..., R[A+3+C] := R[A](R[A+1], R[A+2])
        Op::TForCall => ([(a, 3), none, none], [(a + 4, c), none]),
        // If R[A+4] ~= nil: R[A+2] = R[A+4]; pc -= Bx
        Op::TForLoop => (r1(a + 4), w1(a + 2)),
        // R[A] is the table; R[A+1..A+B] are values to set
        Op::SetList => ([(a, b + 1), none, none], w0),
        Op::Closure => (r0, w1(a)),
        // Writes a variable count starting at R[A]. Conservative: just write R[A].
        Op::Vararg | Op::GetVarg => (r0, w1(a)),
        Op::VargIdx => (r1(c), w1(a)),
        Op::ErrNNil => (r1(a), w0),
    }
}

/// compute the slot indices an op WRITES in the
/// caller's window, with the op's inline depth offset applied.
/// Used by `compute_body_writes` and `compute_live_in_slots`.
pub(super) fn op_writes_at_offset(rop: &RecordedOp, op_offset: u32) -> impl Iterator<Item = u32> {
    let (_r, w) = rw_ranges(rop.inst);
    w.into_iter()
        .flat_map(move |(s, n)| op_offset + s..op_offset + s + n)
}

fn op_reads_at_offset(rop: &RecordedOp, op_offset: u32) -> impl Iterator<Item = u32> {
    let (r, _w) = rw_ranges(rop.inst);
    // register `max_stack` is the lowerer's virtual constant register (see
    // `split_const_operands`), not a slot
    let frame = rop.proto.max_stack as u32;
    r.into_iter()
        .flat_map(|(s, n)| s..s + n)
        .filter(move |&s| s < frame)
        .map(move |s| op_offset + s)
}

/// Sets bit `k`, growing `set` as needed; whether it was clear.
fn insert(set: &mut Vec<bool>, k: u32) -> bool {
    let k = k as usize;
    if k >= set.len() {
        set.resize(k + 1, false);
    }
    !std::mem::replace(&mut set[k], true)
}

fn members(set: &[bool]) -> Vec<u32> {
    (0..set.len() as u32).filter(|&k| set[k as usize]).collect()
}

/// compute the parent body's slot-write set. Walks
/// `record.ops`, applying each op's `inline_depth` offset, and
/// returns a sorted unique list of slot indices that ANY op writes.
/// Stored on `CompiledTrace.body_writes` so child side traces can
/// intersect against it at compile time.
pub fn compute_body_writes(record: &TraceRecord, op_offsets: &[u32]) -> Vec<u32> {
    let mut s = Vec::new();
    for (i, rop) in record.ops.iter().enumerate() {
        let off = op_offsets.get(i).copied().unwrap_or(0);
        for w in op_writes_at_offset(rop, off) {
            insert(&mut s, w);
        }
    }
    members(&s)
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
    let mut written = Vec::new();
    let mut live_in = Vec::new();
    for (i, rop) in record.ops.iter().enumerate() {
        let off = op_offsets.get(i).copied().unwrap_or(0);
        // Reads first — if a slot hasn't been written by a prior op,
        // it's live-in.
        for r in op_reads_at_offset(rop, off) {
            if !written.get(r as usize).copied().unwrap_or(false) {
                insert(&mut live_in, r);
            }
        }
        // Then mark writes (the op's writes happen "after" its reads
        // for purposes of subsequent ops).
        for w in op_writes_at_offset(rop, off) {
            insert(&mut written, w);
        }
    }
    members(&live_in)
}
