//! Which string accumulator the buffered emit takes over.

use super::*;

/// The accumulator the buffered emit takes over: the first bufferable one
/// (both its registers strings when the concatenation reads them) whose
/// accumulator the entry check reads, and whose piece it reads too or is a
/// temporary.
pub(super) fn buffered_accum(
    escape: &EscapeAnalysis,
    head_live: &[bool],
    temps: &[bool],
) -> Option<BufferedAccum> {
    escape
        .accum_sites
        .iter()
        .find(|s| s.state == BufferState::Bufferable)
        .filter(|s| {
            let p = s.piece_slot as usize;
            head_live[s.accum_slot as usize] && (head_live[p] || temps[p])
        })
        .map(|s| BufferedAccum {
            accum_slot: s.accum_slot,
            piece_slot: s.piece_slot,
            pre1_idx: s.op_idx - 2,
            pre2_idx: s.op_idx - 1,
            concat_idx: s.op_idx,
            post_idx: s.has_post.then_some(s.op_idx + 1),
        })
}
