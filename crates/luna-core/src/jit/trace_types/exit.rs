//! Exit slot tags and the encoding of a trace's return value.

use super::*;

/// What tag a register holds at the trace's exit point (relative
/// to its entry tag). Stored per register in `CompiledTrace.exit_tags`
/// so the dispatcher knows how to re-pack the i64 payload back into
/// a `Value` after the trace runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitTag {
    /// Slot is untouched by the trace — restore the entry tag.
    Untouched,
    /// Trace writes an `Int` value to this slot (arith result,
    /// LoadI, Len, ForLoop step / count / visible-var).
    Int,
    /// Trace writes a `Float` bit-pattern to this slot (LoadF
    /// result, Float arith on two Float operands).
    Float,
    /// Trace writes a `Table` ptr to this slot (NewTable result).
    Table,
    /// Trace writes a `Closure` ptr to this slot.
    /// Today the only producer is `Op::GetUpval` whose result is
    /// inferred (via `infer_upval_exit`) to feed an `Op::Call` as
    /// the call target — the upval *must* be a closure for that
    /// dispatch to be sound.
    Closure,
    /// Trace actively writes Nil to this slot (the only
    /// producer today is `Op::LoadNil`; raw payload is 0). The
    /// dispatcher restores `Value::Nil` regardless of the slot's
    /// entry tag. Split out from `Untouched` so a LoadNil writer
    /// over an Int/Float/Table entry slot doesn't get mis-packed
    /// back as the entry type.
    Nil,
    /// Trace writes a `Str` ptr to this slot (LoadK
    /// of a Str constant, Move from a Str slot, or Concat result).
    /// Dispatcher repacks as `Value::Str(Gc::from_ptr(raw))`.
    Str,
    /// Trace writes a boolean: payload 0 (false) or 1 (true); the
    /// dispatcher repacks it with tag `raw::FALSE` plus the payload.
    Bool,
}

/// Derive an [`ExitTag`] vector from a per-slot `RegKind` snapshot.
/// `Unset` slots restore via the dispatcher's entry tags (trace
/// didn't touch them); writers (including `Nil`)
/// translate one-to-one to a tag the dispatcher packs without
/// consulting the entry tag.
/// Fast-path classification of an `exit_tags`
/// vector. Lets the dispatcher's restore loop skip per-slot
/// match-arm dispatch when the entire vector resolves to a
/// single trivial pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagResKind {
    /// Every slot's tag is `Untouched`. The trace didn't override
    /// any slot's exit type; vm.stack already holds the right
    /// values from either marshal-in or trace spill helpers.
    /// Dispatcher skips the restore loop.
    AllUntouched,
    /// Every slot's tag is `Int`. Dispatcher writes
    /// `Value::Int(reg_state[i])` per slot without a match arm.
    AllInt,
    /// Anything else — fall back to the original loop with
    /// per-iter match.
    Mixed,
}

/// Walk an `exit_tags` slice and classify it for the
/// dispatcher fast path.
#[doc(hidden)]
pub fn classify_exit_tags(tags: &[ExitTag]) -> TagResKind {
    if tags.iter().all(|t| matches!(t, ExitTag::Untouched)) {
        return TagResKind::AllUntouched;
    }
    if tags.iter().all(|t| matches!(t, ExitTag::Int)) {
        return TagResKind::AllInt;
    }
    TagResKind::Mixed
}

/// Whether a side trace compiled for `child_entry_tags` may run from the
/// parent exit whose tags are `parent_exit_tags` (the close handler wires
/// it only then). An `Untouched` slot still holds what the parent was
/// entered with, which `parent_compile_entry_tags` names; a child that
/// does not read it ([`ENTRY_TAG_ANY`]) takes it whatever it holds.
pub fn exit_tags_match_entry_tags(
    child_entry_tags: &[u8],
    parent_exit_tags: &[ExitTag],
    parent_compile_entry_tags: &[u8],
) -> bool {
    let n = parent_exit_tags.len();
    if child_entry_tags.len() < n {
        return false;
    }
    for i in 0..n {
        let child = child_entry_tags[i];
        let expected = match parent_exit_tags[i] {
            // the parent left the slot as it found it: a child that does
            // not read it takes it as it is
            ExitTag::Untouched if child == ENTRY_TAG_ANY => continue,
            ExitTag::Untouched => match parent_compile_entry_tags.get(i) {
                // Parent didn't capture an entry tag here (inlined-frame
                // scratch slot), or did not check it on entry: the child
                // cannot know the value's tag.
                None | Some(&ENTRY_TAG_ANY) => return false,
                Some(&t) => t,
            },
            ExitTag::Int => crate::runtime::value::raw::INT,
            ExitTag::Float => crate::runtime::value::raw::FLOAT,
            ExitTag::Table => crate::runtime::value::raw::TABLE,
            ExitTag::Closure => crate::runtime::value::raw::CLOSURE,
            ExitTag::Nil => crate::runtime::value::raw::NIL,
            ExitTag::Str => crate::runtime::value::raw::STR,
            // a trace takes a boolean of either value under `raw::FALSE`
            ExitTag::Bool => crate::runtime::value::raw::FALSE,
        };
        if child != expected {
            return false;
        }
    }
    true
}

/// Set in a trace's return value when bits 32.. hold an index into
/// `per_exit_tags` (see [`decode_exit_shape`]).
pub const EXIT_TAGS_INDEX_BIT: u64 = 1 << 54;
/// Set in a trace's return value to resume at a generic-for's TForLoop
/// with its loop variables as they are on the stack: the TForCall
/// helper wrote them there with their real tags, which need not be the
/// ones the trace's registers were compiled for.
pub const EXIT_KEEP_TFOR_VARS: u64 = 1 << 55;
/// Bits 32..54 of a trace's return value: inline site id + 1, or a
/// `per_exit_tags` index under [`EXIT_TAGS_INDEX_BIT`].
const EXIT_SITE_MASK: u64 = (1 << 22) - 1;

/// Decoded exit shape. Returned by
/// [`decode_exit_shape`]. Carries the per-exit metadata the
/// dispatcher's restore loop needs: the resume PC, the
/// `exit_hit_counts` slot index for the side-trace trigger
/// counter, the per-slot exit-tag array to interpret reg_state
/// through, and a flag for the global classified-restore fast
/// path.
///
/// The lifetime ties `exit_tags_for_pc` to whichever input slice
/// the decode picked from (one of the `CompiledTrace` fields).
/// The dispatcher's per_exit_inline / per_exit_tags / exit_tags
/// Rc clones from the per-dispatch lookup keep them alive for
/// the dispatch.
pub struct DecodedExit<'a> {
    /// Pc the interpreter should resume at after the trace exit.
    pub cont_pc: u32,
    /// Stable id of the exit site (used to key per-site counters / caches).
    pub site_id: u32,
    /// Index into `exit_hit_counts` for the side-trace trigger counter.
    pub exit_hit_idx: usize,
    /// Per-slot exit-tag array describing how to interpret saved register
    /// state for this exit.
    pub exit_tags_for_pc: &'a [ExitTag],
    /// True when the global classified-restore fast path applies.
    pub using_global_exit_tags: bool,
}

/// Decode a trace's i64 return value into the
/// per-exit shape the dispatcher needs to restore vm.stack +
/// bump the hit counter.
///
/// Pure function over the input slices — the dispatcher passes
/// the parent's `per_exit_inline` / `per_exit_tags` / `exit_tags`,
/// or the side trace's same fields when bit 63 of `raw_ret` is set
/// (the side-trace sentinel).
///
/// A depth-0 side exit returns `EXIT_TAGS_INDEX_BIT | (i << 32) | cont_pc`
/// and restores through `per_exit_tags[i]`: several exits can resume at
/// the same pc with different register kinds (and one at `head_pc`, where
/// the clean tail returns too), so the pc alone does not identify the
/// snapshot. A bare `cont_pc` is a return through the trace's own tail and
/// restores through the global tags, even when an exit resumes at that pc
/// as well.
///
/// Layout reminder (from `CompiledTrace::exit_hit_counts`):
/// - `[0..inline.len())` — inline cmp@d>0 sites, indexed by
///   `site_id - 1` (1-based encoding lets `site_id == 0` mean
///   "non-inline").
/// - `[inline.len()..inline.len() + tags.len())` — per_exit_tags,
///   by the index the exit returns.
/// - Last slot — global / clean-tail fallback.
pub fn decode_exit_shape<'a>(
    raw_ret: u64,
    per_exit_inline: &'a [InlineSideExit],
    per_exit_tags: &'a [(u32, TArc<[ExitTag]>)],
    exit_tags: &'a [ExitTag],
) -> DecodedExit<'a> {
    let site_field = ((raw_ret >> 32) & EXIT_SITE_MASK) as u32;
    let cont_pc = (raw_ret & 0xFFFF_FFFF) as u32;
    let inline_n = per_exit_inline.len();
    if raw_ret & EXIT_TAGS_INDEX_BIT != 0 {
        let i = site_field as usize;
        debug_assert_eq!(per_exit_tags[i].0, cont_pc, "per_exit_tags entry's pc");
        return DecodedExit {
            cont_pc,
            site_id: 0,
            exit_hit_idx: inline_n + i,
            exit_tags_for_pc: &per_exit_tags[i].1,
            using_global_exit_tags: false,
        };
    }
    let site_id = site_field;
    if site_id > 0 {
        let idx = (site_id - 1) as usize;
        debug_assert!(
            idx < inline_n,
            "site_idx out of range (idx={} inline_n={})",
            idx,
            inline_n
        );
        debug_assert_eq!(
            per_exit_inline[idx].cont_pc, cont_pc,
            "per_exit_inline entry's cont_pc mismatch with IR"
        );
        DecodedExit {
            cont_pc,
            site_id,
            exit_hit_idx: idx,
            exit_tags_for_pc: &per_exit_inline[idx].exit_tags,
            using_global_exit_tags: false,
        }
    } else {
        DecodedExit {
            cont_pc,
            site_id: 0,
            exit_hit_idx: inline_n + per_exit_tags.len(),
            exit_tags_for_pc: exit_tags,
            using_global_exit_tags: true,
        }
    }
}
