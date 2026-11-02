//! The first pass of a looping body with temporaries, lowered apart.
//!
//! A temporary is a register the body writes before it reads it (see
//! `entry_live`). On the first pass the stack holds what the register held
//! before the loop: the trace does not check it on entry, an exit before
//! the body writes it leaves the stack slot alone, and the collector's root
//! lists leave it out. On the later passes it holds what the pass before
//! left, which an exit before the body writes it again stores back and the
//! root lists name. So the body and its tail are lowered twice, as LuaJIT
//! copies the first iteration of a loop: the first pass's copy in the block
//! the dispatcher enters, the temporaries held on the stack, whose back edge
//! goes to the loop head; and the loop head's copy, the temporaries of the
//! kinds that back edge leaves them, whose own back edge comes round to the
//! loop head again where it leaves them of the same kinds, and leaves the
//! trace otherwise. No block joins the two copies, so the loop moves no
//! value between them on any pass.

use super::*;

/// What lowering the body changes, which the loop head's copy starts from
/// again as the first pass's did.
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
}

/// The loop head's copy of the body, still to lower once the first pass's
/// tail has set the kinds its back edge leaves.
pub(super) struct LoopCopy {
    cx: BodyCx,
    /// the state the first pass's copy started from
    restart: Restart,
}

/// The first pass's copy of the body, in `peel`.
pub(super) fn emit_first_pass<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    mut cx: BodyCx,
    peel: Block,
) -> Option<LoopCopy> {
    let restart = Restart::take(lw);
    lw.bcx.switch_to_block(peel);
    lw.bcx.seal_block(peel);
    emit_pass(lw, pl, &mut cx)?;
    Some(LoopCopy { cx, restart })
}

/// The loop head's copy of the body and its tail, lowered after the first
/// pass's tail: the temporaries of the kinds that tail's back edge leaves
/// them, which this copy's back edge must leave them again (see
/// `loop_kinds_match`).
pub(super) fn emit_loop_copy<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    lc: LoopCopy,
) -> Option<()> {
    let LoopCopy { mut cx, restart } = lc;
    let Some(back) = lw.back_edge_kinds.take() else {
        // nothing loops back: the loop head is never reached
        return Some(());
    };
    restart.put(lw);
    for (r, _) in pl.temps.iter().enumerate().filter(|(_, t)| **t) {
        lw.current_kinds[r] = back[r];
        lw.head_kinds[r] = back[r];
    }
    lw.stored.clear();
    lw.head_stored.clear();
    // not sealed: the tail below adds the back edge
    lw.bcx.switch_to_block(lw.body_loop);
    emit_pass(lw, pl, &mut cx)?;
    emit_tail(lw, pl)?;
    Some(())
}
