//! Which head-frame registers a trace has to check on entry.
//!
//! A frame starts with whatever its register window held before (only
//! missing parameters are cleared, as in PUC `luaD_precall`), so a trace
//! cannot ask for the tags of registers it does not read: the leftovers
//! differ from call to call. The trace checks the registers it reads
//! before writing them; the others start as `RegKind::StackHeld`, whose
//! value is the one on vm.stack.
//!
//! A looping trace checks, besides, every register its body writes. After
//! the first pass such a register no longer matches the stack, yet an exit
//! taken before the body writes it again would leave the stack's stale
//! value; checked on entry, it keeps the kind it has on every pass. (Working
//! out where those exits resume and whether the register is dead there
//! cost more compile time than the check saves.)

use super::slots::rw_ranges;
use super::*;

/// For each of the head frame's `max_stack` registers, whether the trace
/// takes its value (and so its tag) from the entry. Only the ops the
/// lowering emits count: those before `end` and the terminator at `end`.
/// `parent_exit` is the exit a side trace starts from: the side trace can
/// only leave a register to the stack where that exit did.
pub(super) fn entry_live(
    record: &TraceRecord,
    op_offsets: &[u32],
    inline_writes: &[(u32, u32)],
    end: usize,
    max_stack: usize,
    may_loop: bool,
    parent_exit: Option<&[ExitTag]>,
) -> Vec<bool> {
    if record.side_trace_parent.is_some() && parent_exit.is_none() {
        return vec![true; max_stack];
    }
    let ops = &record.ops[..(end + 1).min(record.ops.len())];
    let mut live = vec![false; max_stack];
    // what the lowering writes on every path, so a later read sees the
    // trace's own value
    let mut defined = vec![false; max_stack];
    // what some op may leave with a value the stack does not have
    let mut written = vec![false; max_stack];
    for (i, rop) in ops.iter().enumerate() {
        let off = op_offsets.get(i).copied().unwrap_or(0) as usize;
        let inst = rop.inst;
        let op = inst.op();
        let (reads, writes) = rw_ranges(inst);
        // register `max_stack` is the lowerer's virtual constant register
        // (see `split_const_operands`), not a slot
        let frame = rop.proto.max_stack as u32;
        let mut read = |r: u32| {
            let s = off + r as usize;
            if r < frame && s < max_stack && !defined[s] {
                live[s] = true;
            }
        };
        for &(lo, n) in &reads {
            (lo..lo + n).for_each(&mut read);
        }
        match op {
            // spilled for the closure to capture
            Op::Closure => rop.proto.protos[inst.bx() as usize]
                .upvals
                .iter()
                .filter(|d| d.in_stack)
                .for_each(|d| read(u32::from(d.index))),
            // the ipairs path keeps the previous value. Close needs none:
            // it spills the registers it has a kind for and the helper
            // reads the others off the stack, which holds them
            op if op.is_tfor_call() => read(inst.a() + op.for_layout().map_or(0, |l| l.var()) + 1),
            _ => {}
        }
        let inlined_call = matches!(op, Op::Call)
            && ops
                .get(i + 1)
                .is_some_and(|n| n.inline_depth > rop.inline_depth);
        let sure =
            !(op == Op::TestSet || op.is_for_loop() || op.is_tfor_loop() || op.is_tfor_call())
                && !inlined_call;
        let mut write = |s: usize| {
            if s < max_stack {
                written[s] = true;
                defined[s] |= sure;
            }
        };
        for &(lo, n) in &writes {
            (lo..lo + n).for_each(|w| write(off + w as usize));
        }
        // the values of a call inlined into the trace land in the caller's
        // R[A] on at the callee's return; a vararg expansion writes R[A] on
        if let Some(&(first, n)) = inline_writes.get(i) {
            (first..first + n).for_each(|s| write(s as usize));
        }
        if let Some(lay) = op.for_layout().filter(|_| op.is_tfor_call()) {
            let a = off + inst.a() as usize;
            let first = a + lay.var() as usize;
            [a + lay.control() as usize, first, first + 1]
                .into_iter()
                .for_each(write);
        }
    }
    if let Some(tags) = parent_exit {
        for (s, l) in live.iter_mut().enumerate() {
            if !matches!(tags.get(s), Some(ExitTag::Untouched)) {
                *l = true;
            }
        }
    }
    if may_loop {
        // the body leaves its own value behind; checked on entry instead
        for (s, l) in live.iter_mut().enumerate() {
            *l |= written[s];
        }
    }
    live
}

/// The exit tags of the parent trace's exit a side trace starts from, from
/// the side trace's register 0 on (the frame that exit resumes in; laid out
/// as `exit_hit_counts`: inline exits, tagged exits, then the global one),
/// or `None` for a trace that is not a side trace or whose parent is gone.
pub(super) fn side_parent_exit_tags(record: &TraceRecord) -> Option<Vec<ExitTag>> {
    let (parent_proto, parent_head_pc, idx) = record.side_trace_parent?;
    let traces = parent_proto.traces.borrow();
    let parent = traces.iter().find(|t| t.head_pc == parent_head_pc)?;
    let tags = parent.exit_tags_of(idx);
    let off = parent.exit_frame_offset(idx).min(tags.len());
    Some(tags[off..].to_vec())
}
