//! Where the first pass of a looping trace rejoins the loop (see `peel`).

use super::*;

/// The first op from `from` at which the first pass can rejoin the loop
/// body: one no multi-op form spans (a folded call, the buffered
/// accumulator's four ops, a comparison and the jump it consumes, a skip
/// of a comparison's other way, an inlined call), or `end`, the tail.
pub(super) fn peel_point(
    record: &TraceRecord,
    from: usize,
    end: usize,
    math_folds: &[TraceMathFold],
    accum: Option<&BufferedAccum>,
    consumed_by_cmp: &[bool],
    alt_paths: &[Option<alt_path::AltPath>],
) -> usize {
    if from == 0 {
        return 0;
    }
    let inside = |p: usize, lo: usize, hi: usize| lo < p && p <= hi;
    (from..end)
        .find(|&p| {
            record.ops[p].inline_depth == 0
                && record.ops[p - 1].inline_depth == 0
                && !consumed_by_cmp.get(p).copied().unwrap_or(false)
                && !math_folds
                    .iter()
                    .any(|f| inside(p, f.start_idx, f.call_idx.max(f.start_idx + 3)))
                && !accum.is_some_and(|a| inside(p, a.pre1_idx, a.post_idx.unwrap_or(a.concat_idx)))
                && !alt_paths.iter().enumerate().any(|(i, a)| match a {
                    Some(alt_path::AltPath::Skip { join, .. }) => inside(p, i, *join),
                    _ => false,
                })
        })
        .unwrap_or(end)
}

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
