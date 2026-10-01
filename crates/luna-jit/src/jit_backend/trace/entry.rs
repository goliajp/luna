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

use super::*;
use luna_core::runtime::function::Proto;

/// For each of the head frame's `max_stack` registers, whether the trace
/// takes its value (and so its tag) from the entry. Only the ops the
/// lowering emits count: those before `end` and the terminator at `end`.
/// `parent_exit` is the exit a side trace starts from: the side trace can
/// only leave a register to the stack where that exit did.
pub(super) fn entry_live(
    record: &TraceRecord,
    op_offsets: &[u32],
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
    let mut defined = vec![false; max_stack];
    // the last op that may leave a register with a value the stack does
    // not have
    let mut last_write: Vec<Option<usize>> = vec![None; max_stack];
    for (i, rop) in ops.iter().enumerate() {
        let off = op_offsets.get(i).copied().unwrap_or(0) as usize;
        let inlined_call = matches!(rop.inst.op(), Op::Call)
            && ops
                .get(i + 1)
                .is_some_and(|n| n.inline_depth > rop.inline_depth);
        for r in trace_reads(rop) {
            let s = off + r as usize;
            if s < max_stack && !defined[s] {
                live[s] = true;
            }
        }
        let (_, writes) = op_reads_writes(rop.inst);
        let mut may_write: Vec<usize> = writes.iter().map(|&w| off + w as usize).collect();
        // the value of a call inlined into the trace lands in the caller's
        // R[A] at the callee's Return1, one below the callee's window
        let returned = (matches!(rop.inst.op(), Op::Return1) && rop.inline_depth > 0 && off > 0)
            .then(|| off - 1);
        may_write.extend(returned);
        if matches!(rop.inst.op(), Op::TForCall) {
            let a = off + rop.inst.a() as usize;
            may_write.extend([a + 2, a + 4, a + 5]);
        }
        for &s in &may_write {
            if s < max_stack {
                last_write[s] = Some(i);
            }
        }
        // what the lowering writes on every path, so a later read sees
        // the trace's own value
        let sure: &[usize] = match rop.inst.op() {
            Op::TestSet | Op::ForLoop | Op::TForLoop | Op::TForCall => &[],
            Op::Call if inlined_call => &[],
            Op::Return1 => returned.as_slice(),
            _ => &may_write,
        };
        for &s in sure {
            if s < max_stack {
                defined[s] = true;
            }
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
            *l |= last_write[s].is_some();
        }
    }
    live
}

/// The registers the lowering of `rop` reads, in the op's own frame.
fn trace_reads(rop: &RecordedOp) -> Vec<u32> {
    let inst = rop.inst;
    let a = inst.a();
    let frame = rop.proto.max_stack as u32;
    let (mut r, _) = op_reads_writes(inst);
    match inst.op() {
        // spilled for the closure to capture
        Op::Closure => r.extend(captured_sources(rop.proto, inst.bx() as usize)),
        // Close needs none: it spills the registers it has a kind for and
        // the helper reads the others off the stack, which holds them
        // the ipairs path keeps the previous value
        Op::TForCall => r.push(a + 5),
        _ => {}
    }
    // register `max_stack` is the lowerer's virtual constant register
    // (see `split_const_operands`), not a slot
    r.retain(|&s| s < frame);
    r
}

fn captured_sources(proto: Gc<Proto>, bx: usize) -> impl Iterator<Item = u32> {
    proto.protos[bx]
        .upvals
        .iter()
        .filter(|d| d.in_stack)
        .map(|d| u32::from(d.index))
        .collect::<Vec<_>>()
        .into_iter()
}

/// The exit tags of the parent trace's exit a side trace starts from
/// (laid out as `exit_hit_counts`: inline exits, tagged exits, then the
/// global one), or `None` for a trace that is not a side trace or whose
/// parent is gone.
pub(super) fn side_parent_exit_tags(record: &TraceRecord) -> Option<Vec<ExitTag>> {
    let (parent_proto, parent_head_pc, idx) = record.side_trace_parent?;
    let traces = parent_proto.traces.borrow();
    let parent = traces.iter().find(|t| t.head_pc == parent_head_pc)?;
    let inline_n = parent.per_exit_inline.len();
    let tags_n = parent.per_exit_tags.len();
    let tags: &[ExitTag] = if idx < inline_n {
        &parent.per_exit_inline[idx].exit_tags
    } else if idx < inline_n + tags_n {
        &parent.per_exit_tags[idx - inline_n].1
    } else {
        &parent.exit_tags
    };
    Some(tags.to_vec())
}
