//! surface tests for the accumulator-detection types.
use super::{AccumSite, BufferState, EscapeAnalysis};

#[test]
fn buffer_state_variants_exist() {
    let b = BufferState::Bufferable;
    let nb = BufferState::NonBuffered;
    assert_ne!(b, nb);
}

#[test]
fn accum_site_clones_cleanly() {
    let site = AccumSite {
        op_idx: 0,
        pc: 0,
        accum_slot: 0,
        piece_slot: 1,
        inline_depth: 0,
        state: BufferState::Bufferable,
    };
    let cloned = site.clone();
    assert_eq!(site.op_idx, cloned.op_idx);
    assert_eq!(site.state, cloned.state);
}

#[test]
fn escape_analysis_default_has_empty_accum_fields() {
    let ea: EscapeAnalysis = Default::default();
    assert!(ea.accum_sites.is_empty());
    assert!(ea.accum_live_at_op.is_empty());
}
