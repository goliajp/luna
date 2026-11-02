//! Which head-frame registers a trace has to check on entry.
//!
//! A frame starts with whatever its register window held before (only
//! missing parameters are cleared, as in PUC `luaD_precall`), so a trace
//! cannot ask for the tags of registers it does not read: the leftovers
//! differ from call to call. The trace checks the registers it reads
//! before writing them; the others start as `RegKind::StackHeld`, whose
//! value is the one on vm.stack.
//!
//! A looping trace's body may write a register it does not read first: a
//! temporary. On the first pass the stack holds whatever the register held
//! before (a value of an earlier run, or the nil a collection cleared it
//! to), which the trace must neither check nor write back; on the later
//! passes the register holds what the pass before wrote, which an exit
//! before the body writes it again must write back. So the body is lowered
//! twice (`lower/peel.rs`): once for the first pass, the temporaries held
//! on the stack, and once in the loop, the temporaries of the kind the
//! first pass leaves them. A
//! register a looping body writes only on some ways (a `TestSet`, the loop
//! registers, an inlined call's results) is checked on entry instead, and
//! keeps the kind it has on every pass. So is every register a side trace
//! writes.

use super::slots::rw_ranges;
use super::*;
use luna_core::__private::CompiledTraceInternals;
use luna_core::__private::ProtoTraceInternals;

/// The head frame's registers as the trace takes them on entry.
pub(super) struct HeadRegs {
    /// Per register, whether the trace takes its value (and so its tag)
    /// from the entry.
    pub(super) live: Vec<bool>,
    /// Per register, whether it is a temporary of a looping body.
    pub(super) temps: Vec<bool>,
}

/// For each of the head frame's `max_stack` registers, whether the trace
/// takes its value (and so its tag) from the entry, and the temporaries of
/// a looping body. Only the ops the lowering emits count: those before
/// `end` and the terminator at `end`. `parent_exit` is the exit a side
/// trace starts from: the side trace can only leave a register to the stack
/// where that exit did. `also_live`: registers checked on entry whatever
/// the ops do.
pub(super) fn entry_live(
    record: &TraceRecord,
    op_offsets: &[u32],
    inline_writes: &[(u32, u32)],
    end: usize,
    max_stack: usize,
    may_loop: bool,
    parent_exit: Option<&[ExitTag]>,
    also_live: &[u32],
) -> HeadRegs {
    let mut regs = HeadRegs {
        live: vec![true; max_stack],
        temps: vec![false; max_stack],
    };
    if record.side_trace_parent.is_some() && parent_exit.is_none() {
        return regs;
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
    for &r in also_live {
        if let Some(l) = live.get_mut(r as usize) {
            *l = true;
        }
    }
    if may_loop {
        let root = record.side_trace_parent.is_none();
        for s in 0..max_stack {
            if !written[s] || live[s] {
                continue;
            }
            if root && defined[s] {
                regs.temps[s] = true;
            } else {
                // the body leaves its own value behind; checked on entry
                live[s] = true;
            }
        }
    }
    regs.live = live;
    regs
}

/// The exit tags of the parent trace's exit a side trace starts from, from
/// the side trace's register 0 on (the frame that exit resumes in; laid out
/// as `exit_hit_counts`: inline exits, tagged exits, then the global one),
/// or `None` for a trace that is not a side trace or whose parent is gone.
pub(super) fn side_parent_exit_tags(record: &TraceRecord) -> Option<Vec<ExitTag>> {
    let (parent_proto, parent_head_pc, idx) = record.side_trace_parent?;
    let traces = parent_proto.traces();
    let parent = traces.iter().find(|t| t.head_pc() == parent_head_pc)?;
    let tags = parent.exit_tags_of(idx);
    let off = parent.exit_frame_offset(idx).min(tags.len());
    Some(tags[off..].to_vec())
}
