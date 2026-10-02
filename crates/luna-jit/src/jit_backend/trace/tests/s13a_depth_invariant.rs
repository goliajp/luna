//! pure-function tests for `verify_depth_invariant`.
//! Synthetic `(depth, is_call)` sequences exercise the depth
//! contract without needing a `Gc<Proto>`.
use super::{MAX_INLINE_DEPTH, verify_depth_invariant};

#[test]
fn empty_sequence_is_valid() {
    assert!(verify_depth_invariant(&[]));
}

#[test]
fn single_op_at_depth_zero_is_valid() {
    assert!(verify_depth_invariant(&[(0, false)]));
    assert!(verify_depth_invariant(&[(0, true)]));
}

#[test]
fn single_op_at_nonzero_depth_is_invalid() {
    assert!(!verify_depth_invariant(&[(1, false)]));
    assert!(!verify_depth_invariant(&[(2, true)]));
}

#[test]
fn linear_ascent_one_step_at_a_time_is_valid() {
    // 0 (Call) → 1 (Call) → 2 (Call) → 3
    assert!(verify_depth_invariant(&[
        (0, true),
        (1, true),
        (2, true),
        (3, false),
    ]));
}

#[test]
fn ascent_without_preceding_call_is_invalid() {
    // 0 (not Call) → 1 — violates "depth bump must follow Op::Call".
    assert!(!verify_depth_invariant(&[(0, false), (1, false)]));
}

#[test]
fn ascent_skipping_a_depth_level_is_invalid() {
    // 0 → 2 (skip 1). Even with preceding Op::Call, the recorder
    // must surface every intermediate frame.
    assert!(!verify_depth_invariant(&[(0, true), (2, false)]));
}

#[test]
fn arbitrary_descent_is_valid() {
    // Climb to depth 3, then drop straight to 0 (multi-Return).
    assert!(verify_depth_invariant(&[
        (0, true),
        (1, true),
        (2, true),
        (3, false),
        (0, false),
    ]));
}

#[test]
fn re_ascent_after_descent_is_valid() {
    // 0 (Call) → 1 → 0 → 1 (Call) → 2. Each bump preceded by a
    // Call; descents are unconstrained.
    assert!(verify_depth_invariant(&[
        (0, true),
        (1, false),
        (0, true),
        (1, true),
        (2, false),
    ]));
}

#[test]
fn boundary_at_max_inline_depth_is_valid() {
    // Walk all the way up to MAX_INLINE_DEPTH. Each bump preceded
    // by an Op::Call.
    let mut items: Vec<(u8, bool)> = Vec::new();
    for d in 0..=MAX_INLINE_DEPTH {
        // The op at depth d is an Op::Call iff there's a deeper
        // op to come (it'll push the next frame).
        let is_call = d < MAX_INLINE_DEPTH;
        items.push((d, is_call));
    }
    assert!(verify_depth_invariant(&items));
}

#[test]
fn depth_exceeding_max_inline_depth_is_invalid() {
    // Hit MAX_INLINE_DEPTH + 1. Each step legal, but the cap is
    // exceeded.
    let mut items: Vec<(u8, bool)> = Vec::new();
    for d in 0..=MAX_INLINE_DEPTH {
        items.push((d, true));
    }
    items.push((MAX_INLINE_DEPTH + 1, false));
    assert!(!verify_depth_invariant(&items));
}

#[test]
fn descent_then_ascent_without_call_is_invalid() {
    // 0 (Call) → 1 (not Call) → 0 → 1 — the second ascent's
    // previous op is the depth-0 op that's NOT a Call.
    assert!(!verify_depth_invariant(&[
        (0, true),
        (1, false),
        (0, false),
        (1, false),
    ]));
}
