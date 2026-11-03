//! Where the interpreter went after a recorded op, for the comparisons
//! whose direction the lowerer reads from it.

use super::*;

/// The pc the interpreter ran after op `i` of `record`, and whether the
/// instruction there is a jump: the next recorded op's (the terminator's
/// too, when the comparison is the body's last op); after the last op,
/// in the head frame, the head when the recording closed there (a side
/// trace that went round its loop back to where it started), or the head
/// of the trace a linked side trace reached. `None` otherwise.
pub(super) fn pc_after(
    record: &TraceRecord,
    head_proto: Gc<Proto>,
    i: usize,
) -> Option<(u32, bool)> {
    if let Some(next) = record.ops.get(i + 1) {
        return Some((next.pc, next.inst.op().is_jump()));
    }
    let rop = &record.ops[i];
    if rop.inline_depth != 0 || !rop.proto.ptr_eq(record.head_proto) {
        return None;
    }
    let pc = if record.closed_at_head {
        record.head_pc
    } else {
        record.link_pc?
    };
    let is_jmp = head_proto
        .code
        .get(pc as usize)
        .is_some_and(|x| x.op().is_jump());
    Some((pc, is_jmp))
}
