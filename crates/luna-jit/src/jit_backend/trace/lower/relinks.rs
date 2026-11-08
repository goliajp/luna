//! What a trace's calling exits, its relink block and its gate are emitted
//! from (see `side_call`, `reentry`): taken from the lowering once the rest
//! of the trace is emitted, so that the same code emits them there or, from
//! the instructions the lowering kept, once a side cell is filled (see
//! `super::super::lir::expand`).

use super::*;
use crate::jit_backend::trace::lir::Count;
use cranelift_codegen::ir::{FuncRef, SigRef};
use std::sync::Arc;

/// What the hand-over to the optimizing tier's loop and the way back into
/// the trace read of the trace (see `handoff`).
pub(super) struct Ctx<'a> {
    /// Each register's variable, the inlined functions' past `max_stack`.
    pub(super) regs: &'a [Variable],
    pub(super) max_stack: usize,
    pub(super) reg_state: Value,
    /// The trace functions' signature.
    pub(super) sig: SigRef,
    pub(super) flush: Option<&'a FlushCtx>,
    pub(super) head_pc: u32,
    /// The register window, past which the dispatcher's words sit.
    pub(super) window: usize,
    pub(super) tier: Option<Tier>,
}

/// The trace's iteration count and the cell of its optimizing tier's loop.
#[derive(Clone, Copy)]
pub(super) struct Tier {
    pub(super) count: Count,
    pub(super) at: u32,
    /// The loop cell's address (see `handoff::load_loop_cell`).
    pub(super) loop_cell: i64,
}

impl Tier {
    /// Whether the count is this call's.
    pub(super) fn per_call(&self) -> bool {
        matches!(self.count, Count::Frame(..))
    }
}

/// [`Ctx`] of the trace being lowered: `tier` its iteration count, if any.
pub(super) fn ctx_of<'a>(
    entry: &'a EntryRegs,
    flush: Option<&'a FlushCtx>,
    tier: Option<&TierUp>,
    pl: &Plan<'_>,
) -> Ctx<'a> {
    let tier = tier.map(|t| Tier {
        count: super::handoff::count_at(entry.reg_state, pl, t),
        at: t.at(),
        loop_cell: luna_core::__private::tier_up_loop_cell(t) as *const TCellPtr as i64,
    });
    Ctx {
        regs: &entry.regs_full,
        max_stack: pl.max_stack,
        reg_state: entry.reg_state,
        sig: entry.trace_fn_sig_ref,
        flush,
        head_pc: pl.record.head_pc,
        window: pl.window_size_us,
        tier,
    }
}

/// Where a trace's back edge counts, without the addresses (which another
/// Vm's code has its own of): in the count cell, or in this call's word at
/// `reg_state + off`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Counted {
    Cell,
    Frame(i32),
}

/// The gate between the checks before the loop and the loop (see
/// `reentry::fill_gate`), and where the loop starts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Gate {
    pub(crate) block: Block,
    /// The first pass, or the loop head when there is none.
    pub(crate) start: Block,
    pub(crate) peeled: bool,
}

/// Everything the calling exits' calls, the relink block and the gate are
/// emitted from, kept with a trace's instructions (no address, no handle:
/// numbers of its blocks, variables, values and functions).
#[derive(Clone, Debug)]
pub(crate) struct Relinks {
    pub(crate) regs: Box<[Variable]>,
    pub(crate) max_stack: u32,
    pub(crate) reg_state: Value,
    pub(crate) sig: SigRef,
    pub(crate) flush: Option<FlushCtx>,
    pub(crate) head_pc: u32,
    pub(crate) window: u32,
    /// The loop's count, for a trace whose back edge counts.
    pub(crate) tier: Option<(Counted, u32)>,
    /// `luna_jit_side_cell_list`.
    pub(crate) list_fn: FuncRef,
    /// Whether the back edge counts towards the optimizing tier's loop.
    pub(crate) loops: bool,
    pub(crate) reentry: Block,
    pub(crate) gate: Option<Gate>,
    /// Set by the relink block to the word the exit that relinks left (see
    /// `reentry::count_relink`): in bit 0 whether the side trace's
    /// registers may go on in the optimizing tier's loop, from bit 32 on
    /// the exit's note for the dispatcher.
    pub(crate) loop_ok: Variable,
    /// Set by the relink block: the loop's count is reached, and the gate
    /// moves on to the optimizing tier.
    pub(crate) hot: Variable,
    /// The entry block, which ends in its jump to `reentry`.
    pub(crate) entry: Block,
    /// Per cell, the block of its exit, which ends in its return: an exit
    /// whose call is left to the expansion.
    pub(crate) exits: Box<[Block]>,
    /// Per cell, the typed exit it belongs to.
    pub(crate) cells: Arc<[u32]>,
}

impl Relinks {
    /// Bytes `self` takes, roughly (the cells' list is shared).
    pub(crate) fn size(&self) -> usize {
        size_of::<Relinks>() + 4 * (self.regs.len() + self.exits.len())
    }

    /// [`Ctx`] of the trace, its addresses those of `bcx`'s relocations.
    pub(super) fn ctx<E: Emit>(&self, bcx: &mut E) -> Ctx<'_> {
        let find = |bcx: &mut E, k: RelocKind| {
            let r = bcx.relocs_mut().iter().find(|r| r.0 == k);
            r.expect("a counting loop reads its cells").1
        };
        let tier = self.tier.map(|(c, at)| Tier {
            count: match c {
                Counted::Cell => Count::Cell(find(bcx, RelocKind::TierCell)),
                Counted::Frame(off) => Count::Frame(self.reg_state, off),
            },
            at,
            loop_cell: find(bcx, RelocKind::TierLoop),
        });
        Ctx {
            regs: &self.regs,
            max_stack: self.max_stack as usize,
            reg_state: self.reg_state,
            sig: self.sig,
            flush: self.flush.as_ref(),
            head_pc: self.head_pc,
            window: self.window as usize,
            tier,
        }
    }
}

/// The blocks the calling exits share once emitted: the call block, and the
/// relink block it goes on at.
#[derive(Default)]
pub(crate) struct Way {
    pub(super) call: Option<Block>,
    pub(super) relink: Option<Block>,
}

/// The way back into a trace whose exits may run side traces, while it is
/// lowered.
pub(super) struct Open {
    /// The calls are left to the instructions' expansion (see the module
    /// comment): the exits return, the gate goes on, nothing else is made.
    pub(super) defer: bool,
    pub(super) entry: Block,
    pub(super) reentry: Block,
    pub(super) loop_ok: Variable,
    pub(super) hot: Variable,
    pub(super) way: Way,
    /// The exits whose calls are left to the expansion.
    pub(super) exits: Vec<Block>,
}

/// The [`Relinks`] of the trace `lw` lowered.
pub(super) fn relinks_of<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, o: &Open) -> Relinks {
    let tier = (lw.tier_count.as_ref().filter(|_| lw.loops)).map(|t| {
        let c = match super::handoff::count_at(lw.entry.reg_state, pl, t) {
            Count::Cell(_) => Counted::Cell,
            Count::Frame(_, off) => Counted::Frame(off),
        };
        (c, t.at())
    });
    let gate = lw.blocks.gate.map(|block| Gate {
        block,
        start: lw.blocks.peel.unwrap_or(lw.blocks.body_loop),
        peeled: lw.blocks.peel.is_some(),
    });
    Relinks {
        regs: lw.entry.regs_full.as_slice().into(),
        max_stack: pl.max_stack as u32,
        reg_state: lw.entry.reg_state,
        sig: lw.entry.trace_fn_sig_ref,
        flush: lw.flush_ctx,
        head_pc: pl.record.head_pc,
        window: pl.window_size_us as u32,
        tier,
        list_fn: lw.bcx.import_func(lw.h.rt.side_cell_list_id),
        loops: lw.loops,
        reentry: o.reentry,
        gate,
        loop_ok: o.loop_ok,
        hot: o.hot,
        entry: o.entry,
        exits: o.exits.as_slice().into(),
        cells: lw.side_cells.as_slice().into(),
    }
}
