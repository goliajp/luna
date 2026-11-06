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
//! before the body writes it again must write back. So the ops up to the
//! last temporary's first write are lowered twice (`lower/peel.rs`): once
//! for the first pass, the temporaries held on the stack, and once in the
//! loop, the temporaries of the kind the first pass leaves them. A
//! register a looping body writes only on some ways (a `TestSet`, the loop
//! registers, an inlined call's results) is checked on entry instead, and
//! keeps the kind it has on every pass. So is every register a side trace
//! writes. With `split`, a register the body writes on every path and never
//! reads is no temporary but written-only: not lowered twice, held on the
//! stack on entry, it leaves each exit before its write untouched, and the
//! dispatcher gives it the kind the back edge leaves it once the call has
//! gone round a back edge (`PassCount::PerCall`).

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
    /// The ops before this one are lowered for the first pass apart: the
    /// op after the last temporary's first write; 0 without temporaries.
    pub(super) peel_end: usize,
    /// Per register, whether it is written-only (see the module doc).
    pub(super) write_only: Vec<bool>,
}

/// For each of the head frame's `max_stack` registers, whether the trace
/// takes its value (and so its tag) from the entry, and the temporaries of
/// a looping body. Only the ops the lowering emits count: those before
/// `end` and the terminator at `end`. `parent_exit` is the exit a side
/// trace starts from: the side trace can only leave a register to the stack
/// where that exit did. `also_live`: registers checked on entry whatever
/// the ops do. `split`: set written-only registers apart from the
/// temporaries.
#[allow(clippy::too_many_arguments)]
pub(super) fn entry_live(
    record: &TraceRecord,
    op_offsets: &[u32],
    inline_writes: &[(u32, u32)],
    end: usize,
    max_stack: usize,
    may_loop: bool,
    parent_exit: Option<&[ExitTag]>,
    also_live: &[u32],
    split: bool,
) -> HeadRegs {
    let mut regs = HeadRegs::new(max_stack);
    if record.side_trace_parent.is_some() && parent_exit.is_none() {
        return regs;
    }
    let ops = &record.ops[..(end + 1).min(record.ops.len())];
    let mut scan = Scan::new(max_stack);
    for i in 0..ops.len() {
        let off = op_offsets.get(i).copied().unwrap_or(0) as usize;
        scan.op(ops, i, off, inline_writes.get(i).copied());
    }
    if let Some(tags) = parent_exit {
        for (s, l) in scan.live.iter_mut().enumerate() {
            if !matches!(tags.get(s), Some(ExitTag::Untouched)) {
                *l = true;
            }
        }
    }
    for &r in also_live {
        if let Some(l) = scan.live.get_mut(r as usize) {
            *l = true;
        }
    }
    if may_loop {
        classify_written(
            &mut scan,
            &mut regs,
            record.side_trace_parent.is_none(),
            split,
        );
    }
    regs.peel_end = regs.peel_end.min(end);
    regs.live = scan.live;
    regs
}

/// What the ops up to the terminator do to the head frame's registers.
struct Scan {
    /// Read before the lowering writes them on every path.
    live: Vec<bool>,
    /// What the lowering writes on every path, so a later read sees the
    /// trace's own value.
    defined: Vec<bool>,
    /// What some op may leave with a value the stack does not have.
    written: Vec<bool>,
    /// The op that first writes a register on every path.
    first_def: Vec<usize>,
    /// Read by some op.
    read: Vec<bool>,
}

impl HeadRegs {
    /// Every register taken from the entry, none apart.
    fn new(max_stack: usize) -> HeadRegs {
        HeadRegs {
            live: vec![true; max_stack],
            temps: vec![false; max_stack],
            peel_end: 0,
            write_only: vec![false; max_stack],
        }
    }
}

impl Scan {
    fn new(max_stack: usize) -> Scan {
        Scan {
            live: vec![false; max_stack],
            defined: vec![false; max_stack],
            written: vec![false; max_stack],
            first_def: vec![usize::MAX; max_stack],
            read: vec![false; max_stack],
        }
    }

    /// Notes what `ops[i]`, whose frame starts at register `off`, reads and
    /// writes; `inline_write`: the registers it writes as an inlined call's
    /// return or a vararg expansion.
    fn op(&mut self, ops: &[RecordedOp], i: usize, off: usize, inline_write: Option<(u32, u32)>) {
        let rop = &ops[i];
        let inst = rop.inst;
        let op = inst.op();
        let writes = self.read_op(rop, off);
        let inlined_call = matches!(op, Op::Call)
            && ops
                .get(i + 1)
                .is_some_and(|n| n.inline_depth > rop.inline_depth);
        let sure =
            !(op == Op::TestSet || op.is_for_loop() || op.is_tfor_loop() || op.is_tfor_call())
                && !inlined_call;
        let mut write = |s: usize| self.write(s, i, sure);
        for &(lo, n) in &writes {
            (lo..lo + n).for_each(|w| write(off + w as usize));
        }
        // the values of a call inlined into the trace land in the caller's
        // R[A] on at the callee's return; a vararg expansion writes R[A] on
        if let Some((first, n)) = inline_write {
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

    /// Notes the registers `rop`, whose frame starts at register `off`,
    /// reads; returns the ranges it writes.
    fn read_op(&mut self, rop: &RecordedOp, off: usize) -> [(u32, u32); 2] {
        let inst = rop.inst;
        let op = inst.op();
        let (reads, writes) = rw_ranges(inst);
        // register `max_stack` is the lowerer's virtual constant register
        // (see `split_const_operands`), not a slot
        let frame = rop.proto.max_stack as u32;
        let mut read = |r: u32| {
            let s = off + r as usize;
            if r < frame && s < self.read.len() {
                self.read[s] = true;
            }
            if r < frame && s < self.live.len() && !self.defined[s] {
                self.live[s] = true;
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
        writes
    }

    /// Notes that op `i` writes register `s`, on every path if `sure`.
    fn write(&mut self, s: usize, i: usize, sure: bool) {
        if s < self.written.len() {
            self.written[s] = true;
            if sure && !self.defined[s] {
                self.defined[s] = true;
                self.first_def[s] = i;
            }
        }
    }
}

/// Sorts the registers a looping body writes without reading them first:
/// in a root trace, those it writes on every path are temporaries, lowered
/// for the first pass apart up to the op after their first write, or with
/// `split` written-only when nothing reads them; the others (and every one
/// a side trace writes) leave the body's own value behind and are checked
/// on entry.
fn classify_written(scan: &mut Scan, regs: &mut HeadRegs, root: bool, split: bool) {
    for s in 0..scan.live.len() {
        if !scan.written[s] || scan.live[s] {
            continue;
        }
        if root && scan.defined[s] && split && !scan.read[s] {
            regs.write_only[s] = true;
        } else if root && scan.defined[s] {
            regs.temps[s] = true;
            regs.peel_end = regs.peel_end.max(scan.first_def[s] + 1);
        } else {
            // the body leaves its own value behind; checked on entry
            scan.live[s] = true;
        }
    }
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
