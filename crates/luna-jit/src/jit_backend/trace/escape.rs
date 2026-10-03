use super::*;

/// Escape state per NewTable site recorded in a trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeState {
    /// No observed use forces heap residency; emit may sink.
    Sinkable,
    /// A use forced materialisation — call argument, return value,
    /// stored into another sinkable table, observed at a side-exit.
    Escaped,
}

/// one NewTable site found in a recorded trace, tagged
/// with the slot it lives in and the final [`EscapeState`].
#[derive(Debug, Clone)]
pub struct AllocSite {
    /// Index into `record.ops` of the NewTable op.
    pub op_idx: usize,
    /// Bytecode PC of the NewTable (in the site's enclosing Proto).
    pub pc: u32,
    /// Destination register `R[A]` = newly-allocated table.
    pub a: u32,
    /// Inline depth at the time of the NewTable (0 = trace head).
    pub inline_depth: u8,
    /// Array capacity decoded from NewTable.B (`B = 0` for hash-only
    /// sites gives array_cap = 0).
    pub array_cap: u32,
    /// unique string-key const indices touched by
    /// `Op::SetField` / `Op::GetField` on this site (in scan order).
    /// `virt_vars` indices [array_cap .. array_cap + hash_keys.len())
    /// hold the hash slots; SetFieldSunkWrite / GetFieldSunkRead
    /// look up the key's position in this vec to pick the slot.
    pub hash_keys: Vec<u32>,
    /// Final escape-state classification after the sweep.
    pub state: EscapeState,
}

/// per-op action recorded by the escape sweep when an op
/// reads or writes through a NewTable site. Emit consults this to
/// take the sunk path (no helper call, virtual `Variable`s) instead
/// of the heap-alloc helper path. Only set for ops whose sunk path
/// is implemented. The sweep marks the site Escaped on any
/// unsupported op so emit can rely on "Sinkable → all-ops sunk".
#[derive(Debug, Clone, Copy)]
pub enum OpAction {
    /// `Op::NewTable` allocating site_idx. Emit skips the
    /// `luna_jit_new_table` helper; the site lives as virtual slots.
    NewTableSite {
        /// Allocation site index in [`EscapeAnalysis::sites`].
        site_idx: u32,
    },
    /// `Op::SetList` writing into a sunk site's array. Emit
    /// `def_var`s the source registers into virtual slots.
    SetListWrite {
        /// Allocation site index.
        site_idx: u32,
    },
    /// `Op::GetI` reading from a sunk site at a 1-based key. Emit
    /// `def_var`s the corresponding virtual slot into the dst reg.
    GetIRead {
        /// Allocation site index.
        site_idx: u32,
        /// 1-based array key being read.
        key: u32,
    },
    /// `Op::SetI` writing into a sunk site's slot at a
    /// 1-based key. Emit `def_var`s the value register into the
    /// matching virt slot Variable and updates `virt_kinds` so the
    /// next `GetI` reads the right RegKind. The value source is
    /// the runtime register `R[C]`.
    SetISunkWrite {
        /// Allocation site index.
        site_idx: u32,
        /// 1-based array key being written.
        key: u32,
    },
    /// `Op::SetTable` writing into a sunk site's slot
    /// at a 1-based key const-folded from a backward scan of the
    /// trace (LoadI → Move chain → key). Same emit shape as
    /// SetISunkWrite; the key field is the resolved int.
    SetTableSunkWrite {
        /// Allocation site index.
        site_idx: u32,
        /// Const-folded 1-based array key.
        key: u32,
    },
    /// `Op::SetField` writing into a sunk site's
    /// hash slot. `hash_slot` is the position of the key's const
    /// index in `AllocSite.hash_keys`. virt_vars index is
    /// `array_cap + hash_slot`.
    SetFieldSunkWrite {
        /// Allocation site index.
        site_idx: u32,
        /// Position in [`AllocSite::hash_keys`] of the field key.
        hash_slot: u32,
    },
    /// `Op::GetField` reading from a sunk site's
    /// hash slot. Same indexing as SetFieldSunkWrite.
    GetFieldSunkRead {
        /// Allocation site index.
        site_idx: u32,
        /// Position in [`AllocSite::hash_keys`] of the field key.
        hash_slot: u32,
    },
}

/// One register bound to a sunk site: the site's index in
/// [`EscapeAnalysis::sites`] and the register, in the site's frame,
/// that holds the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveBinding {
    /// Allocation site index.
    pub site: u32,
    /// Frame-relative register.
    pub reg: u32,
}

/// buffer state for a candidate `Op::Concat`
/// accumulator. Mirrors [`EscapeState`] semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState {
    /// No observed use forces materialisation of the buffered slot;
    /// emit may take the buffered path (per-trace `Vec<u8>` append).
    Bufferable,
    /// A use forced materialisation — accumulator passed to a Call,
    /// Move'd elsewhere, observed at a side-exit with Str exit_tag,
    /// etc. Emit falls back to the existing `luna_jit_op_concat`
    /// helper path.
    NonBuffered,
}

/// one `Op::Concat A B=2` candidate found by
/// `detect_accumulators` where `A` (the destination) is the same
/// register as the first operand AND survives across the trace's
/// back-edge.
#[derive(Debug, Clone)]
pub struct AccumSite {
    /// Index into `record.ops` of the `Op::Concat` op.
    pub op_idx: usize,
    /// Bytecode PC of the `Op::Concat`.
    pub pc: u32,
    /// The accumulator slot (= `Op::Concat.A`). Also reads as first
    /// operand and writes the result.
    pub accum_slot: u32,
    /// The piece slot (= `Op::Concat.A + 1` since `B = 2`).
    pub piece_slot: u32,
    /// Inline depth; the detector only accepts 0.
    pub inline_depth: u8,
    /// Final buffer-state classification after escape-style sweep.
    pub state: BufferState,
}

/// result of the post-recording, pre-emit escape sweep
/// run by [`try_compile_trace_with_options`]. Emit reads `sites`
/// (for the Sinkable list) and `op_actions` (per-op dispatch hint),
/// and consumes `live_at_op` at every cmp side-exit emit point to
/// materialise the right virt slots.
#[derive(Debug, Default)]
pub struct EscapeAnalysis {
    /// One [`AllocSite`] per `Op::NewTable` in the trace.
    pub sites: Vec<AllocSite>,
    /// Per-op action, length = `effective_end`. Indexed by op index
    /// in `record.ops`. `None` for ops the sweep didn't tag.
    pub op_actions: Vec<Option<OpAction>>,
    /// Per-op snapshot of the registers bound to a site BEFORE this op
    /// processes, length = `effective_end`. Read at exit emit sites:
    /// each listed site is materialised into a heap `Gc<Table>` once
    /// and written into every register listed for it (a `Move` makes
    /// more than one register hold the same table). Sites that end up
    /// Escaped after the sweep are still in the snapshot but emit gates
    /// on the final state.
    pub live_at_op: Vec<Vec<LiveBinding>>,
    /// Accumulator sites identified in this trace.
    pub accum_sites: Vec<AccumSite>,
    /// per-op snapshot of bound accumulator-site
    /// indices, parallel to `live_at_op`. Length = `effective_end`.
    pub accum_live_at_op: Vec<Vec<u32>>,
}

impl EscapeAnalysis {
    /// Count of sites whose final state is [`EscapeState::Sinkable`].
    pub fn sinkable_count(&self) -> u32 {
        self.sites
            .iter()
            .filter(|s| s.state == EscapeState::Sinkable)
            .count() as u32
    }
}

/// does this op write the register at `R[A]` as its
/// sole / primary destination? Used by `const_fold_int_key` to
/// recognise the last writer of a key register. Conservative:
/// excludes ops that write multiple regs (Call, ForLoop) or that
/// don't write a reg at all (Set*, control flow). The const-fold
/// scan returns `None` on any unrecognised writer.
pub(super) fn writes_target_a(op: Op) -> bool {
    matches!(
        op,
        Op::Move
            | Op::LoadI
            | Op::LoadF
            | Op::LoadK
            | Op::LoadNil
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::IDiv
            | Op::Mod
            | Op::Pow
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr
            | Op::Unm
            | Op::BNot
            | Op::Len
            | Op::NewTable
            | Op::GetI
            | Op::GetTable
            | Op::GetUpval
            | Op::GetField
            | Op::GetTabUp
            | Op::Closure
    )
}

/// walk backward from `set_table_idx` looking for the
/// most recent writer of `reg` at the same `inline_depth`. If it's
/// `LoadI sbx` with `sbx in 1..=cap`, the key is the const `sbx`.
/// `Move R[?] = R[src]` chains the search to `src` (up to
/// `MAX_STEPS` hops). Any other writer kills the trail → `None`.
pub(super) fn const_fold_int_key(
    record: &TraceRecord,
    set_table_idx: usize,
    reg: u32,
    cap: u32,
) -> Option<u32> {
    const MAX_STEPS: usize = 8;
    let depth = record.ops[set_table_idx].inline_depth;
    let mut cur_reg = reg;
    let mut steps = 0;
    let mut j = set_table_idx;
    while j > 0 && steps < MAX_STEPS {
        j -= 1;
        steps += 1;
        let rop = &record.ops[j];
        if rop.inline_depth != depth {
            continue;
        }
        let inst = rop.inst;
        if inst.a() != cur_reg || !writes_target_a(inst.op()) {
            continue;
        }
        match inst.op() {
            Op::LoadI => {
                let sbx = inst.sbx();
                if sbx >= 1 && (sbx as u32) <= cap {
                    return Some(sbx as u32);
                }
                return None;
            }
            Op::Move => {
                cur_reg = inst.b();
                continue;
            }
            _ => return None,
        }
    }
    None
}
