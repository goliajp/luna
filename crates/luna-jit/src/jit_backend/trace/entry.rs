//! Which head-frame registers a trace has to check on entry.
//!
//! A frame starts with whatever its register window held before (only
//! missing parameters are cleared, as in PUC `luaD_precall`), so a trace
//! cannot ask for the tags of registers it does not read: the leftovers
//! differ from call to call. The trace checks the registers it reads
//! before writing them; the others start as `RegKind::StackHeld`, whose
//! value is the one on vm.stack.
//!
//! A looping trace needs one more thing. After the first pass a held
//! register the body writes no longer matches the stack, yet an exit
//! taken before the body writes it again restores it as held, i.e.
//! leaves the stack's stale value. That is fine only where the
//! interpreter does not read the register before writing it, so such a
//! register is checked on entry instead unless it is dead at every place
//! those exits resume.

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
        for s in 0..max_stack {
            if live[s] {
                continue;
            }
            if let Some(last) = last_write[s] {
                live[s] = !ops[..=last].iter().all(|rop| {
                    rop.inline_depth == 0
                        && resume_pcs(rop.proto, rop.pc)
                            .into_iter()
                            .chain([record.head_pc])
                            .all(|pc| dead_at(rop.proto, pc, s as u32))
                });
            }
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

/// Every pc an exit taken at the op at `pc` can resume the interpreter at:
/// the op itself (a guard redoes it) and, for an op that branches, where
/// each way leads (a comparison's other branch, a loop's exit).
fn resume_pcs(proto: Gc<Proto>, pc: u32) -> Vec<u32> {
    let code = &proto.code;
    let p = pc as i64;
    let mut out = vec![p];
    let at = |q: i64| code.get(q as usize).copied();
    if let Some(inst) = at(p) {
        match inst.op() {
            Op::Eq
            | Op::Lt
            | Op::Le
            | Op::EqK
            | Op::EqI
            | Op::LtI
            | Op::LeI
            | Op::GtI
            | Op::GeI
            | Op::Test
            | Op::TestSet => {
                out.extend([p + 1, p + 2]);
                if let Some(next) = at(p + 1)
                    && matches!(next.op(), Op::Jmp)
                {
                    out.push(p + 2 + next.sj() as i64);
                }
            }
            Op::Jmp => out.push(p + 1 + inst.sj() as i64),
            Op::ForLoop | Op::TForLoop => out.extend([p + 1, p + 1 - inst.bx() as i64]),
            // an exit right after the iterator call resumes at TForLoop
            Op::TForCall => out.push(p + 1),
            _ => {}
        }
    }
    out.into_iter()
        .filter(|&q| q >= 0 && (q as usize) < code.len())
        .map(|q| q as u32)
        .collect()
}

/// How far `dead_at` looks before it calls a register live.
const DEAD_SCAN_BUDGET: usize = 256;

/// Whether, from `pc` on, register `s` of a frame running `proto` is
/// written before anything reads it on every path. `false` when not
/// shown within [`DEAD_SCAN_BUDGET`] instructions.
pub(super) fn dead_at(proto: Gc<Proto>, pc: u32, s: u32) -> bool {
    // a captured local is read through its upvalue by any call
    let captured = proto.protos.iter().any(|p| {
        p.upvals
            .iter()
            .any(|d| d.in_stack && u32::from(d.index) == s)
    });
    if captured {
        return false;
    }
    let code = &proto.code;
    let mut seen = vec![false; code.len()];
    let mut todo = vec![pc as i64];
    let mut budget = DEAD_SCAN_BUDGET;
    while let Some(p) = todo.pop() {
        if p < 0 || p as usize >= code.len() {
            return false;
        }
        if std::mem::replace(&mut seen[p as usize], true) {
            continue;
        }
        if budget == 0 {
            return false;
        }
        budget -= 1;
        match step(proto, code[p as usize], s) {
            Step::Read => return false,
            Step::Written => {}
            Step::Next(succ) => todo.extend(succ.into_iter().map(|d| p + d)),
        }
    }
    true
}

enum Step {
    /// the op may read the register before writing it
    Read,
    /// the op writes it (or the frame ends) on every path
    Written,
    /// neither: continue at these offsets from the op
    Next(Vec<i64>),
}

fn step(proto: Gc<Proto>, inst: Inst, s: u32) -> Step {
    let a = inst.a();
    let (b, c) = (inst.b(), inst.c());
    let reads = |r: &[u32]| r.contains(&s);
    let from = |lo: u32, n: u32| s >= lo && s < lo + n;
    match inst.op() {
        Op::Jmp => Step::Next(vec![1 + inst.sj() as i64]),
        Op::Eq | Op::Lt | Op::Le => step_branch(reads(&[a, b])),
        Op::EqK | Op::EqI | Op::LtI | Op::LeI | Op::GtI | Op::GeI | Op::Test => step_branch(s == a),
        // the copy happens on one path only
        Op::TestSet => step_branch(s == b),
        Op::LFalseSkip if s == a => Step::Written,
        Op::LFalseSkip => Step::Next(vec![2]),
        Op::ForLoop => {
            if from(a, 3) {
                Step::Read
            } else if s == a + 3 {
                // set before the body runs again; out of scope after it
                Step::Written
            } else {
                Step::Next(vec![1, 1 - inst.bx() as i64])
            }
        }
        Op::TForLoop => {
            if s == a + 4 {
                Step::Read
            } else {
                Step::Next(vec![1, 1 - inst.bx() as i64])
            }
        }
        Op::ForPrep => Step::Read,
        Op::TForPrep if s == a + 3 => Step::Read,
        Op::TForPrep => Step::Next(vec![1 + inst.bx() as i64]),
        Op::TForCall => {
            if from(a, 4) {
                Step::Read
            } else if s >= a + 4 {
                // the iterator's frame and results take everything above
                Step::Written
            } else {
                Step::Next(vec![1])
            }
        }
        Op::Call => {
            if b == 0 || c == 0 || from(a, b) {
                Step::Read
            } else if s >= a {
                // results, and above them registers the call consumed
                Step::Written
            } else {
                Step::Next(vec![1])
            }
        }
        // with k set they close upvalues and to-be-closed values first
        Op::TailCall => step_end(inst.k() || b == 0 || from(a, b)),
        Op::Return => step_end(inst.k() || b == 0 || from(a, b - 1)),
        Op::Return0 => Step::Written,
        Op::Return1 => step_end(s == a),
        Op::Close if s >= a => Step::Read,
        Op::Tbc if s == a => Step::Read,
        Op::SetList if b == 0 || from(a, b + 1) => Step::Read,
        Op::Vararg if c == 0 => {
            if s >= a {
                Step::Read
            } else {
                Step::Next(vec![1])
            }
        }
        Op::Vararg if from(a, c - 1) => Step::Written,
        Op::Vararg => Step::Next(vec![1]),
        Op::Closure if captured_sources(proto, inst.bx() as usize).any(|r| r == s) => Step::Read,
        _ => {
            let (r, w) = op_reads_writes(inst);
            if reads(&r) {
                Step::Read
            } else if w.contains(&s) {
                Step::Written
            } else {
                Step::Next(vec![1])
            }
        }
    }
}

fn step_branch(read: bool) -> Step {
    if read {
        Step::Read
    } else {
        Step::Next(vec![1, 2])
    }
}

fn step_end(read: bool) -> Step {
    if read { Step::Read } else { Step::Written }
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
