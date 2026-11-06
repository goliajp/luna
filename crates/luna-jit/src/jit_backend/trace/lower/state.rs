//! The emit pass's state while it lowers a trace.

use super::*;

/// The trace function under construction and everything the emit pass
/// tracks while lowering it.
pub(super) struct Lower<E: Emit> {
    pub(super) bcx: E,
    pub(super) h: Helpers,
    /// What the entry block set up (see `emit_entry`).
    pub(super) entry: EntryRegs,
    /// The blocks before the body and the loop head.
    pub(super) blocks: HeadBlocks,
    /// The kinds the back edge leaves the head-frame registers with, once
    /// it is emitted.
    pub(super) back_edge_kinds: Option<Vec<RegKind>>,
    /// The kinds the body is lowered for: a back edge runs it again only
    /// when the caller window holds these (see `loop_kinds_match`).
    pub(super) head_kinds: Vec<RegKind>,
    /// The AOT data this trace has defined: `declare_data` gives one name
    /// the same `DataId` again, but `define_data` refuses a second
    /// definition.
    pub(super) defined_aot_data: std::collections::HashSet<DataId>,
    pub(super) escape: EscapeAnalysis,
    pub(super) flush_ctx: Option<FlushCtx>,
    pub(super) virt_vars: Vec<Option<Vec<Variable>>>,
    pub(super) virt_kinds: Vec<Option<Vec<RegKind>>>,
    pub(super) sunk_alloc_seen: u32,
    /// The side exits that rebuild a sunk allocation (a count of emit
    /// sites, for diagnostics).
    pub(super) materialize_emit_count: u32,
    pub(super) closure_seen: u32,
    /// What reg_state holds for each register at the loop head: on entry
    /// the values the prelude loaded (caller window) or the zeroes the
    /// dispatcher filled it with (inline frames); on the back edge what
    /// `sync_reg_state` wrote before the jump.
    pub(super) stored: Vec<Option<Value>>,
    /// Commit register writes to reg_state only where something reads it
    /// (`commit.rs`), keeping values in machine registers across the back
    /// edge: for the optimizing tiers, whose register allocators make use
    /// of that; the baseline tier's would spill.
    pub(super) at_exits: bool,
    /// With `at_exits`, per register: the value reg_state holds for it at
    /// the loop head whichever way the head is reached (`None`: the body
    /// writes the register and the back edge does not store it).
    pub(super) head_stored: Vec<Option<Value>>,
    /// Per register, its kind as lowered so far: at the head, the recorded
    /// entry tag of a head-frame register the trace reads before writing
    /// (the dispatcher checks it), stack-held for the other head-frame
    /// registers, unset past the head frame; then the virtual registers,
    /// which hold a kind only while an op that reads them is lowered.
    pub(super) current_kinds: Vec<RegKind>,
    pub(super) dispatchable: bool,
    /// The first emit-pass site that made the trace not dispatchable,
    /// which `CompiledTraceParts::dispatch_off_reason` reports.
    pub(super) dispatch_off_reason: Option<&'static str>,
    /// The register kinds at each side exit, taken where the exit is
    /// emitted, so a later writer does not change what the exit restores.
    pub(super) per_exit_kinds: ExitKinds,
    /// Each side exit of an inlined comparison: its pc, its encoded site,
    /// the kinds and the frames to rebuild.
    pub(super) per_exit_inline_vec: Vec<(u32, u32, Vec<RegKind>, TArc<[FrameMaterializeInfo]>)>,
    /// The inlined self-recursive calls being lowered, innermost last:
    /// the frames an exit inside them rebuilds.
    pub(super) call_chain: Vec<FrameMaterializeInfo>,
    /// The upvalues read so far.
    pub(super) upvals: UpvalReads,
    /// The constants registers hold so far.
    pub(super) known: Known,
    /// The window slots the virtual registers of the op being lowered
    /// stand in, with the kinds the slots had (see `enter_virt`): an exit
    /// taken inside the op snapshots those kinds, not the virtual ones.
    pub(super) virt_held: Vec<(usize, RegKind)>,
    /// Blocks the other way of a comparison jumps to, by the recorded op
    /// it rejoins at, with the registers the skipped ops write.
    pub(super) alt_joins: std::collections::HashMap<usize, (Block, Vec<u32>)>,
    /// The head-frame registers whose table is tested before the loop head
    /// instead of at each store (see `readonly`).
    pub(super) ro_invariant: Vec<bool>,
    /// The table values tested since the last op that may have run host
    /// code.
    pub(super) ro_checked: Vec<Value>,
    /// The trace's move to the optimizing tier, whose iteration count the
    /// back edge keeps and whose loop it calls (see `handoff`).
    pub(super) tier_count: Option<Box<TierUp>>,
    /// The kinds the back edge leaves the written-only registers (see
    /// `entry_live`); `Err` when one is not a kind the dispatcher may give
    /// them past a back edge, or two back edges disagree.
    pub(super) write_only_kinds: Result<Vec<(u16, ExitTag)>, ()>,
    /// A back edge counts in this call's words past the window
    /// (`handoff::count_at` gave `Count::Frame`).
    pub(super) counts_per_call: bool,
    /// The block the optimizing tier's loop starts at (see `handoff`).
    pub(super) loop_entry: Option<Block>,
    /// A back edge with an iteration count was emitted.
    pub(super) loops: bool,
    /// The count's address, read on entry by a trace that may check (`gc_due`).
    pub(super) due_words: Option<Variable>,
    /// Every way to the op being lowered passed a check for a collection
    /// with nothing that may allocate since (see `gc_due`).
    pub(super) gc_quiet: bool,
}

impl<E: Emit> Lower<E> {
    /// Where the body starts after the prechecks.
    #[inline]
    pub(super) fn body_start(&self) -> Block {
        self.blocks.peel.unwrap_or(self.blocks.body_loop)
    }

    /// The variable of register `i` (of the window and the inline frames).
    #[inline]
    pub(super) fn reg(&self, i: usize) -> Variable {
        self.entry.regs_full[i]
    }
}

/// What reg_state holds for each register at the loop head on entry: the
/// values the prelude loaded.
pub(super) fn entry_values<E: Emit>(lw: &mut Lower<E>) -> Vec<Option<Value>> {
    let (regs, bcx) = (&lw.entry.regs_full, &mut lw.bcx);
    regs.iter().map(|&r| Some(bcx.use_var(r))).collect()
}

/// The upvalues a trace has read so far.
#[derive(Default, Clone)]
pub(super) struct UpvalReads {
    /// Per upvalue index, the variable holding its value once read: the
    /// running closure stays the same through a dispatch. Ordered maps, so
    /// whatever walks them emits in the same order in every process.
    pub(super) cache: std::collections::BTreeMap<u32, Variable>,
    /// Per upvalue index, the checked typed read: its variable, out slot,
    /// check result and type.
    pub(super) checked:
        std::collections::BTreeMap<u32, (Variable, cranelift_codegen::ir::StackSlot, Value, u8)>,
    /// The upvalues whose checked read has been guarded.
    pub(super) check_done: Vec<u32>,
    /// The entry closure, fetched at the first inlined call, which
    /// dominates every later one (the trace is linear).
    pub(super) head_closure: Option<Variable>,
}

/// The constants registers hold at this point of the pass.
#[derive(Clone)]
pub(super) struct Known {
    /// The integer constants (from `LoadI` / `LoadK` earlier in the pass),
    /// so a `//`, `%` or shift by one needs no guard.
    pub(super) int: Vec<Option<i64>>,
    /// The registers holding a string constant loaded earlier: a table
    /// access by such a key may use the slot the recording found it in.
    pub(super) str: Vec<bool>,
}

impl Known {
    /// Nothing known yet of `n` registers.
    pub(super) fn new(n: usize) -> Known {
        Known {
            int: vec![None; n],
            str: vec![false; n],
        }
    }

    /// The integer constant register `i` holds, if one is known.
    pub(super) fn int_at(&self, i: usize) -> Option<i64> {
        self.int.get(i).copied().flatten()
    }
}
