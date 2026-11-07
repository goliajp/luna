//! The first pass of a looping body with temporaries, lowered apart.
//!
//! A temporary is a register the body writes before it reads it (see
//! `entry_live`). On the first pass the stack holds what the register held
//! before the loop: the trace does not check it on entry, an exit before
//! the body writes it leaves the stack slot alone, and the collector's root
//! lists leave it out. On the later passes it holds what the pass before
//! left, which an exit before the body writes it again stores back and the
//! root lists name. So the ops up to the last temporary's first write, `P`,
//! are lowered twice: in the first-pass block the dispatcher enters, the
//! temporaries held on the stack, and at the loop head, the temporaries of
//! the kinds the back edge leaves them (a scratch register may hold a
//! table at `P` and an integer at the back edge). Both go on at one block
//! for op `P`, so the first pass's copy is lowered first, then the rest
//! and the tail, then the loop head's copy, which joins the block where
//! the state there agrees with what the rest was lowered for, and leaves
//! the trace at op `P` otherwise. The loop costs no instruction more per
//! pass, as LuaJIT's copy of the first iteration.

use super::*;

/// What lowering the ops changes, which the loop head's copy of the first
/// ops starts from again as the first pass's did.
struct Restart {
    current_kinds: Vec<RegKind>,
    known: Known,
    upvals: UpvalReads,
    ro_checked: Vec<Value>,
    gc_quiet: bool,
    virt_kinds: Vec<Option<Vec<RegKind>>>,
    closure_seen: u32,
}

impl Restart {
    fn take<E: Emit>(lw: &Lower<E>) -> Restart {
        Restart {
            current_kinds: lw.current_kinds.clone(),
            known: lw.known.clone(),
            upvals: lw.upvals.clone(),
            ro_checked: lw.ro_checked.clone(),
            gc_quiet: lw.gc_quiet,
            virt_kinds: lw.virt_kinds.clone(),
            closure_seen: lw.closure_seen,
        }
    }

    fn put<E: Emit>(self, lw: &mut Lower<E>) {
        lw.current_kinds = self.current_kinds;
        lw.known = self.known;
        lw.upvals = self.upvals;
        lw.ro_checked = self.ro_checked;
        lw.gc_quiet = self.gc_quiet;
        lw.virt_kinds = self.virt_kinds;
        lw.closure_seen = self.closure_seen;
    }

    /// Whether `self`, the loop head's copy at `P`, holds what the ops
    /// after `P` were lowered for, `at`: the same kinds, sunk fields and
    /// upvalue reads, and at least the constants and quiet collector they
    /// took as known (they take no table as tested: see `emit_first_pass`).
    fn holds(&self, at: &Restart) -> bool {
        let keys = |u: &UpvalReads| {
            let c: Vec<u32> = u.cache.keys().copied().collect();
            let k: Vec<u32> = u.checked.keys().copied().collect();
            let mut d = u.check_done.clone();
            d.sort_unstable();
            (c, k, d, u.head_closure.is_some())
        };
        let known = at
            .known
            .int
            .iter()
            .zip(&self.known.int)
            .all(|(a, b)| a.is_none() || a == b)
            && at
                .known
                .str
                .iter()
                .zip(&self.known.str)
                .all(|(a, b)| !a || *b);
        self.current_kinds == at.current_kinds
            && self.virt_kinds == at.virt_kinds
            && keys(&self.upvals) == keys(&at.upvals)
            && known
            && (self.gc_quiet || !at.gc_quiet)
    }
}

/// The loop head's copy of the first ops, still to lower once the tail has
/// set the kinds the back edge leaves.
pub(super) struct LoopCopy {
    cx: BodyCx,
    /// the block op `P` is lowered in
    join: Block,
    /// the state the first pass's copy started from
    restart: Restart,
    /// the state the ops after `P` were lowered for
    at: Restart,
    /// per register, whether the ops after `P` take reg_state to hold it
    stored: Vec<bool>,
}

/// The body of a trace with temporaries, but for the loop head's copy of
/// the first ops: the first pass's copy in `peel`, and the rest.
pub(super) fn emit_first_pass<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    mut cx: BodyCx,
    peel: Block,
) -> Option<LoopCopy> {
    let p = pl.peel_end;
    let restart = Restart::take(lw);
    lw.bcx.switch_to_block(peel);
    lw.bcx.seal_block(peel);
    emit_copy(lw, pl, &mut cx, p)?;
    // what reg_state holds of the registers at `P`
    let stored: Vec<bool> = (0..lw.stored.len())
        .map(|r| {
            let v = use_var_resolved(&mut lw.bcx, lw.entry.regs_full[r]);
            lw.stored[r] == Some(v)
        })
        .collect();
    let join = lw.bcx.create_block();
    lw.bcx.ins().jump(join, &[]);
    // not sealed: the loop head's copy jumps here too
    lw.bcx.switch_to_block(join);
    for (r, s) in stored.iter().enumerate() {
        lw.stored[r] = s.then(|| use_var_resolved(&mut lw.bcx, lw.entry.regs_full[r]));
    }
    // the tested tables are known by their values in one copy, which the
    // other copy's do not match: none is tested at the join, as where the
    // ways of a comparison rejoin
    lw.ro_checked.clear();
    let at = Restart::take(lw);
    emit_ops(lw, pl, &mut cx, p..pl.effective_end)?;
    Some(LoopCopy {
        cx,
        join,
        restart,
        at,
        stored,
    })
}

/// The loop head's copy of the first ops, lowered after the tail: the
/// temporaries of the kinds the back edge leaves them. Leaves the state as
/// the tail left it.
pub(super) fn emit_loop_copy<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    lc: LoopCopy,
) -> Option<()> {
    let LoopCopy {
        mut cx,
        join,
        restart,
        at,
        stored,
    } = lc;
    let Some(back) = lw.back_edge_kinds.clone() else {
        // nothing loops back: the loop head is never reached
        lw.bcx.seal_block(join);
        return Some(());
    };
    let p = pl.peel_end;
    let tail = Restart::take(lw);
    let tail_stored = std::mem::take(&mut lw.stored);
    let tail_head_stored = std::mem::take(&mut lw.head_stored);
    restart.put(lw);
    for (r, _) in pl.temps.iter().enumerate().filter(|(_, t)| **t) {
        lw.current_kinds[r] = back[r];
    }
    // sealed by the tail, which emitted every back edge
    lw.bcx.switch_to_block(lw.blocks.body_loop);
    emit_copy(lw, pl, &mut cx, p)?;
    join_loop_copy(lw, pl, &at, &stored, join);
    lw.bcx.seal_block(join);
    tail.put(lw);
    lw.stored = tail_stored;
    lw.head_stored = tail_head_stored;
    Some(())
}

/// The ops before `p`, as each copy lowers them: from the commit at the
/// start to the commit before the join.
fn emit_copy<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, cx: &mut BodyCx, p: usize) -> Option<()> {
    commit_start(lw, pl);
    emit_ops(lw, pl, cx, 0..p)?;
    commit_at_join(lw, pl, cx, p);
    Some(())
}

/// The end of the loop's copy: the join, when it reached `P` in the state
/// the first pass did (`at`; `stored`, what reg_state held there), the
/// registers stored where the first pass left them; an exit otherwise.
fn join_loop_copy<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    at: &Restart,
    stored: &[bool],
    join: Block,
) {
    let looped = Restart::take(lw);
    if looped.holds(at) && lw.alt_joins.is_empty() && lw.call_chain.is_empty() {
        alias_upvals(lw, &at.upvals, &looped.upvals);
        for (r, _) in stored.iter().enumerate().filter(|(_, s)| **s) {
            let v = use_var_resolved(&mut lw.bcx, lw.entry.regs_full[r]);
            if lw.stored[r] != Some(v) {
                let reg_state = lw.entry.reg_state;
                lw.bcx
                    .ins()
                    .store(MemFlagsData::new(), v, reg_state, (r as i32) * 8);
            }
        }
        lw.bcx.ins().jump(join, &[]);
    } else {
        // the loop leaves where its copy would not go on as the first did
        checkpoint("peel:loop-copy-differs");
        let p = pl.peel_end;
        let pc = pl.record.ops.get(p).map_or(pl.record.head_pc, |o| o.pc);
        guard_exit(lw, pl, pc, p);
    }
}

/// The commit of op `P` (or the one before the tail, when the copy is the
/// whole body), emitted before the join in both copies where the values are
/// still at hand: after the join they would be read back from the
/// variables' homes on every pass.
fn commit_at_join<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, cx: &BodyCx, p: usize) {
    if p < pl.effective_end {
        cx.commit_before(lw, pl, p);
    } else {
        commit_end(lw, pl, p);
    }
}

/// Gives the variables the ops after `P` read the upvalues through (`at`)
/// the values the loop head's copy read into its own.
fn alias_upvals<E: Emit>(lw: &mut Lower<E>, at: &UpvalReads, looped: &UpvalReads) {
    let mut pairs: Vec<(Variable, Variable)> = Vec::new();
    for (k, &v) in &at.cache {
        pairs.push((v, looped.cache[k]));
    }
    for (k, c) in &at.checked {
        pairs.push((c.0, looped.checked[k].0));
    }
    if let (Some(a), Some(b)) = (at.head_closure, looped.head_closure) {
        pairs.push((a, b));
    }
    for (a, b) in pairs.into_iter().filter(|(a, b)| a != b) {
        let v = lw.bcx.use_var(b);
        lw.bcx.def_var(a, v);
    }
}
