//! What a baseline trace moves to the optimizing tier from.

use super::image::TraceImage;
use super::*;
use std::sync::Arc;

/// What a baseline trace moves to the optimizing tier from: its
/// instructions, this Vm's values of their relocations, the image it
/// shares (to take the optimizing tier's code from, or give it to), and
/// the record it was lowered from, when there is one, to lower it again
/// for the optimizing tier ([`TierSource::optimizing_lir`]).
pub(crate) struct TierSource {
    pub(crate) lir: Arc<lir::Lir>,
    pub(crate) relocs: Vec<(RelocKind, i64)>,
    pub(crate) image: Option<Arc<TraceImage>>,
    pub(crate) record: Option<TierRecord>,
}

/// A trace's record and the options it was lowered with.
pub(crate) struct TierRecord {
    pub(crate) record: TraceRecord,
    pub(crate) opts: CompileOptions,
    pub(crate) float_only: bool,
}

// SAFETY: the record's function handles are only read, on the thread of
// the Vm that owns the trace (the tier-up runs there; only the lowered
// instructions go to the LLVM compile thread), and the trace keeps the
// functions alive as long as it holds the record
unsafe impl Send for TierRecord {}
// SAFETY: as above; nothing mutates the record
unsafe impl Sync for TierRecord {}

impl TierSource {
    /// The instructions for the optimizing tier, with their relocations:
    /// the record lowered again keeping values in registers to the exits
    /// (`Lower::at_exits`), or the baseline tier's when there is no record
    /// (a trace taken from another Vm's image) or the two lowerings'
    /// relocations do not line up.
    pub(crate) fn optimizing_lir(&self) -> (Arc<lir::Lir>, Vec<(RelocKind, i64)>) {
        if let Some(r) = &self.record {
            // the baseline tier's options: the same relocations (its
            // iteration count among them), which the optimizing tier's
            // code generators read past
            if let Some((lir, _)) = lower::lower_trace_lir(&r.record, r.opts, r.float_only, true) {
                // the relocations name the same things in the same order;
                // their values are the baseline trace's, which the trace
                // keeps alive (this lowering's own frame chains go with
                // the trace it made, dropped here)
                let same = lir.relocs.len() == self.relocs.len()
                    && lir.relocs.iter().zip(&self.relocs).all(|(a, b)| a.0 == b.0);
                let owned = same.then(|| Arc::new(lir.detach()));
                lir.give();
                if let Some(owned) = owned {
                    return (owned, self.relocs.clone());
                }
            }
        }
        (self.lir.clone(), self.relocs.clone())
    }
}
