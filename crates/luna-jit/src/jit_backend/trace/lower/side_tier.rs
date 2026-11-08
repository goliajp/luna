//! When a side trace moves to the optimizing tier: once what its passes
//! cost in the baseline tier's code beyond what they would at the
//! optimizing tier adds up to what the move costs, the usual rule for
//! paying for something whose use is not known ahead. Compiling costs a
//! fixed part plus a part per LIR instruction, and each pass of a side
//! trace (straight code, no loop) runs all of its instructions, each a
//! little cheaper at the optimizing tier: so the move is earned back after
//! `(c0 + a * L) / (s * L)` passes for `L` instructions.

use luna_core::__private::{CompiledTraceParts, TraceRecord};
use luna_core::jit::trace::TIER_UP_THRESHOLD;

/// `a / s`: the cycles the optimizing tier takes to compile one LIR
/// instruction (about 5.4k) over the cycles it saves on one instruction in
/// one pass (about 0.12).
const PER_INSTRUCTION: u64 = 45_400;

/// `c0 / s`: the fixed cycles of a compile (about 130k) over the same
/// saving.
const FIXED: u64 = 1_090_000;

/// `parts`, lowered from `record` to `len` LIR instructions: for a side
/// trace, moving once the move has paid for itself, scaled by `tier_up_at`
/// against its default as a root trace's move is, so that a lower one
/// moves it sooner.
pub(super) fn at_payback(
    record: &TraceRecord,
    len: usize,
    mut parts: CompiledTraceParts,
    tier_up_at: u32,
) -> CompiledTraceParts {
    if record.side_trace_parent.is_some()
        && let Some(t) = parts.tier_up.as_mut()
    {
        let passes = PER_INSTRUCTION + FIXED / len as u64;
        let at = passes * u64::from(tier_up_at) / u64::from(TIER_UP_THRESHOLD);
        t.set_payback_at(at.clamp(1, u64::from(u32::MAX)) as u32);
    }
    parts
}
