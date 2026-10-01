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

use super::slots::rw_ranges;
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
    let pending: Vec<usize> = (0..max_stack)
        .filter(|&s| !live[s] && last_write[s].is_some())
        .collect();
    if may_loop && !pending.is_empty() {
        let resume: Vec<Vec<u32>> = ops
            .iter()
            .map(|rop| {
                if rop.inline_depth == 0 {
                    resume_pcs(rop.proto, rop.pc)
                } else {
                    Vec::new()
                }
            })
            .collect();
        let roots: Vec<u32> = resume
            .iter()
            .flatten()
            .copied()
            .chain([record.head_pc])
            .collect();
        let lv = Liveness::of(record.head_proto, &roots);
        // what is live where an exit taken at or before each op can resume
        let mut at_exit = lv.at(record.head_pc);
        let mut inlined = false;
        let mut upto: Vec<(Set, bool)> = Vec::with_capacity(ops.len());
        for (rop, pcs) in ops.iter().zip(&resume) {
            inlined |= rop.inline_depth > 0;
            for &pc in pcs {
                at_exit = at_exit.union(lv.at(pc));
            }
            upto.push((at_exit, inlined));
        }
        for s in pending {
            let (set, inlined) = upto[last_write[s].expect("pending slots are written")];
            live[s] = inlined || set.has(s);
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

/// A set of a frame's registers.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Set([u64; 4]);

impl Set {
    const ALL: Set = Set([u64::MAX; 4]);
    fn has(self, s: usize) -> bool {
        s < 256 && self.0[s / 64] >> (s % 64) & 1 == 1
    }
    fn with(mut self, s: u32) -> Set {
        if s < 256 {
            self.0[s as usize / 64] |= 1 << (s % 64);
        }
        self
    }
    /// Registers `lo .. lo + n` (clipped to the 256 a frame can have).
    fn range(lo: u32, n: u32) -> Set {
        let hi = lo.saturating_add(n).min(256);
        let lo = lo.min(hi);
        // bits below `x`, word by word
        let below = |x: u32| -> [u64; 4] {
            std::array::from_fn(|i| {
                let base = i as u32 * 64;
                if x >= base + 64 {
                    u64::MAX
                } else if x <= base {
                    0
                } else {
                    (1u64 << (x - base)) - 1
                }
            })
        };
        let (h, l) = (below(hi), below(lo));
        Set(std::array::from_fn(|i| h[i] & !l[i]))
    }
    fn from(lo: u32) -> Set {
        Set::range(lo, 256)
    }
    fn union(self, o: Set) -> Set {
        Set(std::array::from_fn(|i| self.0[i] | o.0[i]))
    }
    fn minus(self, o: Set) -> Set {
        Set(std::array::from_fn(|i| self.0[i] & !o.0[i]))
    }
}

/// A function longer than this is not analysed: every register a looping
/// trace writes without reading first is then checked on entry, which
/// keeps the compile time of a trace in a big chunk bounded.
const MAX_ANALYSED_CODE: usize = 4096;

/// Which registers each instruction of a function may read before writing
/// them on some path from it (backward liveness over the whole function).
/// A register a nested function captures counts as live everywhere: any
/// call can read it through the upvalue.
struct Liveness {
    live_in: Vec<Set>,
    captured: Set,
}

impl Liveness {
    /// Computed for the instructions reachable from `roots` only: those
    /// are all that liveness at `roots` depends on.
    fn of(proto: Gc<Proto>, roots: &[u32]) -> Liveness {
        let code = &proto.code;
        let captured = proto
            .protos
            .iter()
            .flat_map(|p| p.upvals.iter())
            .filter(|d| d.in_stack)
            .fold(Set::default(), |acc, d| acc.with(u32::from(d.index)));
        if code.len() > MAX_ANALYSED_CODE {
            return Liveness {
                live_in: vec![Set::ALL; code.len()],
                captured,
            };
        }
        let mut reach = vec![false; code.len()];
        let mut todo: Vec<i64> = roots.iter().map(|&p| i64::from(p)).collect();
        while let Some(p) = todo.pop() {
            if p < 0 || p as usize >= code.len() || std::mem::replace(&mut reach[p as usize], true)
            {
                continue;
            }
            todo.extend(
                successors(code[p as usize])
                    .into_iter()
                    .flatten()
                    .map(|d| p + d),
            );
        }
        let order: Vec<usize> = (0..code.len()).rev().filter(|&p| reach[p]).collect();
        let rw: Vec<(Set, Set)> = code
            .iter()
            .enumerate()
            .map(|(p, &inst)| {
                if !reach[p] {
                    return (Set::default(), Set::default());
                }
                let (r, w) = rw_ranges(inst);
                (runs(&r), runs(&w))
            })
            .collect();
        let mut live_in = vec![Set::default(); code.len()];
        loop {
            let mut changed = false;
            for &p in &order {
                let new = transfer(proto, code[p], p as i64, rw[p], |q| {
                    if q < 0 || q as usize >= code.len() {
                        Set::ALL
                    } else {
                        live_in[q as usize]
                    }
                });
                if new != live_in[p] {
                    live_in[p] = new;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Liveness { live_in, captured }
    }

    fn at(&self, pc: u32) -> Set {
        self.live_in
            .get(pc as usize)
            .copied()
            .unwrap_or(Set::ALL)
            .union(self.captured)
    }
}

fn runs(v: &[(u32, u32)]) -> Set {
    v.iter()
        .fold(Set::default(), |acc, &(lo, n)| acc.union(Set::range(lo, n)))
}

/// Where control goes after `inst`, as offsets from its pc (`transfer`
/// reads the live sets there); `None` past an op that ends the frame or
/// whose result does not depend on what follows.
fn successors(inst: Inst) -> [Option<i64>; 2] {
    match inst.op() {
        Op::Jmp => [Some(1 + inst.sj() as i64), None],
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
        | Op::TestSet => [Some(1), Some(2)],
        Op::LFalseSkip => [Some(2), None],
        Op::ForLoop | Op::TForLoop => [Some(1), Some(1 - inst.bx() as i64)],
        Op::TForPrep => [Some(1 + inst.bx() as i64), None],
        Op::ForPrep | Op::TailCall | Op::Return | Op::Return0 | Op::Return1 => [None, None],
        Op::Call if inst.b() == 0 || inst.c() == 0 => [None, None],
        Op::SetList if inst.b() == 0 => [None, None],
        _ => [Some(1), None],
    }
}

/// The registers live before `inst` (at `p`) given those live before
/// each instruction (`out`): read before written on some path.
fn transfer(
    proto: Gc<Proto>,
    inst: Inst,
    p: i64,
    (r, w): (Set, Set),
    out: impl Fn(i64) -> Set,
) -> Set {
    let a = inst.a();
    let (b, c) = (inst.b(), inst.c());
    let both = |x: i64, y: i64| out(p + x).union(out(p + y));
    let reg = |n: u32| Set::default().with(n);
    match inst.op() {
        Op::Jmp => out(p + 1 + inst.sj() as i64),
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
        // the copy happens on one path only
        | Op::TestSet => r.union(both(1, 2)),
        Op::LFalseSkip => out(p + 2).minus(reg(a)),
        // R[A+3] is set before the body runs again and out of scope after
        Op::ForLoop => Set::range(a, 3).union(both(1, 1 - inst.bx() as i64).minus(reg(a + 3))),
        Op::TForLoop => reg(a + 4).union(both(1, 1 - inst.bx() as i64)),
        Op::ForPrep => Set::ALL,
        Op::TForPrep => reg(a + 3).union(out(p + 1 + inst.bx() as i64)),
        // the iterator's frame and results take everything above R[A+3]
        Op::TForCall => Set::range(a, 4).union(out(p + 1).minus(Set::from(a + 4))),
        Op::Call if b == 0 || c == 0 => Set::ALL,
        // results, and above them registers the call consumed
        Op::Call => Set::range(a, b).union(out(p + 1).minus(Set::from(a))),
        // with k set they close upvalues and to-be-closed values first
        Op::TailCall | Op::Return if inst.k() || b == 0 => Set::ALL,
        Op::TailCall => Set::range(a, b),
        Op::Return => Set::range(a, b - 1),
        Op::Return0 => Set::default(),
        Op::Return1 => reg(a),
        Op::Close => Set::from(a).union(out(p + 1)),
        Op::Tbc => reg(a).union(out(p + 1)),
        Op::SetList if b == 0 => Set::ALL,
        Op::SetList => Set::range(a, b + 1).union(out(p + 1)),
        Op::Vararg if c == 0 => Set::from(a).union(out(p + 1)),
        Op::Vararg => out(p + 1).minus(Set::range(a, c - 1)),
        Op::Closure => captured_sources(proto, inst.bx() as usize)
            .fold(out(p + 1).minus(reg(a)), Set::with),
        _ => r.union(out(p + 1).minus(w)),
    }
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

#[cfg(test)]
mod tests {
    use super::Set;

    #[test]
    fn range_holds_exactly_its_registers() {
        for lo in [0u32, 1, 5, 63, 64, 65, 127, 128, 200, 255, 256, 300] {
            for n in [0u32, 1, 2, 3, 63, 64, 65, 128, 256, u32::MAX] {
                let r = Set::range(lo, n);
                for s in 0..256usize {
                    let want =
                        (s as u64) >= u64::from(lo) && (s as u64) < u64::from(lo) + u64::from(n);
                    assert_eq!(r.has(s), want, "lo {lo} n {n} s {s}");
                }
            }
        }
    }
}
