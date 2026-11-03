mod end;

use super::*;
pub(super) use end::{TraceEnd, plain_trace_end};

/// per-op register window offset.
///
/// For a recorded trace with inline self-recursive `Op::Call`s, each
/// `RecordedOp` at depth `d` views its `R[k]` as the trace's
/// `reg_state_buf[offset_per_op[i] + k]`. The offset accumulates
/// across self-recursive calls:
/// - depth 0 ops: offset = 0
/// - When `Op::Call A B C` (self-recursive) at depth d is followed
///   by an op at depth d+1: that callee's offset = `offset[d] + A + 1`.
///   (Lua call ABI: callee's `R[0]` lives at caller's `R[A+1]`, since
///   `R[A]` is the function value being called.)
/// - On `Op::Return*` at depth d>0: depth drops to d-1, offset
///   reverts to the saved `offset[d-1]`.
///
/// Used by the body emit to address registers across inlined
/// frames. The companion `enclosing_call_a` Vec gives the matching
/// caller `Op::Call`'s A field for any depth>0 op (None at depth 0),
/// which the `Op::Return*` emit consumes to compute the
/// caller's destination slot for return-value copy-back.
/// pure-function depth invariant verifier.
///
/// Returns `true` iff the recorded op sequence's inline-depth
/// trail is well-formed for `compute_op_offsets` to consume:
///
/// 1. The first op (if any) is at depth 0.
/// 2. Any depth bump (`d_curr > d_prev`) is exactly `d_prev + 1`
///    AND the previous op is an `Op::Call` (the only legitimate
///    frame-push trigger in Lua bytecode under the recorder's
///    self-rec inline contract).
/// 3. Depth never exceeds `MAX_INLINE_DEPTH` (the lowerer's
///    window-size cap).
///
/// Depth descents (`d_curr < d_prev`) are unconstrained — the
/// interpreter can unwind many frames via consecutive
/// `Op::Return*` / tail-call exits, and the offset stack `pop()`
/// loop in `compute_op_offsets` handles arbitrary descent.
///
/// The function operates on `(depth, is_call)` tuples instead of
/// the heavier `RecordedOp` so the lib unit tests can construct
/// synthetic input without a real `Gc<Proto>`.
pub(crate) fn verify_depth_invariant(items: &[(u8, bool)]) -> bool {
    if items.is_empty() {
        return true;
    }
    if items[0].0 != 0 {
        return false;
    }
    let mut prev_depth = items[0].0;
    let mut prev_was_call = items[0].1;
    for &(d, is_call) in &items[1..] {
        if d > prev_depth {
            if d != prev_depth + 1 {
                return false;
            }
            if !prev_was_call {
                return false;
            }
        }
        if d > MAX_INLINE_DEPTH {
            return false;
        }
        prev_depth = d;
        prev_was_call = is_call;
    }
    true
}

/// Whether the `Op::Call` at `i` is lowered inline: the recorder followed
/// it into a Lua function (the next op is one level deeper) and
/// [`inline_calls`] could lay out the callee's frame: the argument count
/// is fixed or the recording fixes the stack top it comes from, the callee
/// returns a count the recording fixes, and it does not need the
/// arguments as a table. A vararg callee's extra arguments move below its
/// registers as `push_frame` moves them, in the trace's registers and in
/// the frames the frame-materialise helper rebuilds at an exit.
///
/// Any other call ends the trace there, as a call the trace leaves to the
/// interpreter.
pub(super) fn call_inlinable(record: &TraceRecord, calls: &[Option<InlineCall>], i: usize) -> bool {
    let depth = record.ops[i].inline_depth as usize;
    calls.get(i).copied().flatten().is_some()
        && record
            .ops
            .get(i + 1)
            .is_some_and(|next| next.inline_depth as usize == depth + 1)
        && depth < MAX_INLINE_DEPTH as usize
}

#[cfg(test)]
pub(super) fn compute_op_offsets(record: &TraceRecord) -> (Vec<u32>, Vec<Option<u8>>) {
    compute_op_offsets_with(record, &inline_calls(record).0)
}

/// [`compute_op_offsets`] given the record's [`inline_calls`].
pub(super) fn compute_op_offsets_with(
    record: &TraceRecord,
    calls: &[Option<InlineCall>],
) -> (Vec<u32>, Vec<Option<u8>>) {
    let n = record.ops.len();
    let mut offsets = Vec::with_capacity(n);
    let mut enclosing_call_a = Vec::with_capacity(n);
    // `offset_stack[d]` = the register-window offset for depth d.
    // `call_a_stack[d]` = the Op::Call A field that entered depth d.
    //   `call_a_stack[0]` is unused (depth 0 has no enclosing call).
    let mut offset_stack: Vec<u32> = vec![0u32];
    let mut call_a_stack: Vec<u8> = vec![0u8];
    for (i, rop) in record.ops.iter().enumerate() {
        let d = rop.inline_depth as usize;
        if d >= offset_stack.len() {
            // Depth increased — the previous op must be Op::Call
            // (recorder invariant for self-recursive entry).
            debug_assert!(i > 0, "first recorded op cannot be at depth > 0");
            debug_assert!(
                d == offset_stack.len(),
                "depth jumped more than 1 in a single transition"
            );
            let caller_idx = i - 1;
            let caller = &record.ops[caller_idx];
            debug_assert!(
                matches!(caller.inst.op(), Op::Call),
                "depth bump must follow Op::Call"
            );
            let caller_offset = offset_stack[offset_stack.len() - 1];
            let caller_a = caller.inst.a();
            // a vararg callee's extra arguments sit below its registers
            let extras = calls[caller_idx].map_or(0, |c| c.n_varargs);
            let new_offset = caller_offset + caller_a + 1 + extras;
            offset_stack.push(new_offset);
            // Lua register indices fit in u8 by VM design; this
            // cast is lossless for any valid bytecode.
            call_a_stack.push(caller_a as u8);
        } else {
            // Depth decreased (or stayed equal). Pop down to d.
            while offset_stack.len() > d + 1 {
                offset_stack.pop();
                call_a_stack.pop();
            }
        }
        offsets.push(offset_stack[d]);
        enclosing_call_a.push(if d == 0 { None } else { Some(call_a_stack[d]) });
    }
    (offsets, enclosing_call_a)
}

/// Direction the cmp/Jmp pair took in the recording. Both
/// directions can compile, but the brif's predicate and the
/// side-exit PC flip between them.
#[derive(Clone, Copy, Debug)]
pub(super) enum CmpDir {
    /// Recorded: cmp matched K → no pc++ → Jmp executed. Next
    /// recorded op is the Jmp at `cmp_pc + 1`; consumed_by_cmp
    /// marks it. Side-exit PC = `cmp_pc + 2` (interp pc++).
    /// Standard repeat-until / `for` exit-cmp shape.
    TookJmp,
    /// Recorded: cmp didn't match K → pc++ → Jmp skipped. Next
    /// recorded op is the body op at `cmp_pc + 2`. Side-exit PC
    /// = the Jmp's target. Standard `while cond do` body-entry
    /// shape.
    SkippedJmp,
}
