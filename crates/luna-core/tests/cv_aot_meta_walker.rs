//! v2.0 Phase 5 CV gap fill — `jit::aot_meta` walker error paths.
//!
//! Existing inline tests in `aot_meta.rs` cover round-trip happy
//! paths + magic / version mismatch + payload truncation + v3
//! chain misalignment + v1/v2 shape tolerance. The audit-flagged
//! gap is **the per-tail truncation arms** — the v2 / v3 walkers
//! each have multiple "shorter than X" return-points and the
//! decoder is the load-bearing wire-format contract for deploy-side
//! `luna-aot` install. Each malformed shape MUST produce a stable
//! `Err(&'static str)` (deploy walker skips entry + logs) rather
//! than panic or mis-install.
//!
//! Tests here cover the per-error-message arms not already
//! exercised inline:
//!
//! - "blob shorter than header"
//! - "v2 tail truncated at count"
//! - "v2 tail truncated at entry header"
//! - "v2 tail truncated at entry tags"
//! - "v3 tail truncated at count"
//! - "v3 tail truncated at entry header"
//! - "v3 tail truncated at entry tags"
//! - "v3 tail truncated at chain header"
//! - "v3 tail truncated at chain bytes"

use luna_core::jit::aot_meta::{
    AOT_META_MAGIC, AOT_META_VERSION, AotTraceMetaHeader, decode_meta_blob,
};

/// Helper — build the canonical 28-byte header with all-zero
/// payload markers. Caller appends the malformed tail.
fn empty_header() -> Vec<u8> {
    let header = AotTraceMetaHeader {
        magic: AOT_META_MAGIC,
        version: AOT_META_VERSION,
        head_pc: 0,
        n_ops: 0,
        window_size: 0,
        dispatchable: 0,
        tag_res_kind: 0,
        entry_tags_len: 0,
        exit_tags_len: 0,
    };
    let mut b = Vec::with_capacity(AotTraceMetaHeader::SIZE);
    b.extend_from_slice(&header.magic.to_le_bytes());
    b.extend_from_slice(&header.version.to_le_bytes());
    b.extend_from_slice(&header.head_pc.to_le_bytes());
    b.extend_from_slice(&header.n_ops.to_le_bytes());
    b.extend_from_slice(&header.window_size.to_le_bytes());
    b.push(header.dispatchable);
    b.push(header.tag_res_kind);
    b.extend_from_slice(&header.entry_tags_len.to_le_bytes());
    b.extend_from_slice(&header.exit_tags_len.to_le_bytes());
    b
}

/// A 7-byte buffer is way too short to even contain a header
/// (`AotTraceMetaHeader::SIZE == 28`). The decoder's very first
/// length check should reject before touching any field.
#[test]
fn decode_rejects_blob_shorter_than_header() {
    let blob = vec![0u8; 7];
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("shorter than header"),
        "expected 'shorter than header' err, got {err:?}"
    );
}

/// Header present, payload absent, then a single dangling byte
/// where the v2 count u32 should be. The walker reads the first
/// byte past payload-end as part of a u32 check and must reject.
#[test]
fn decode_rejects_v2_tail_truncated_at_count() {
    let mut blob = empty_header();
    // 1 byte of garbage where v2 tail count (u32) would start.
    blob.push(0);
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("v2 tail truncated at count"),
        "expected v2-count truncation err, got {err:?}"
    );
}

/// Header + v2 count=1, but the per-entry header (cont_pc:u32 +
/// tags_len:u32 = 8 bytes) is short.
#[test]
fn decode_rejects_v2_tail_truncated_at_entry_header() {
    let mut blob = empty_header();
    blob.extend_from_slice(&1u32.to_le_bytes()); // count=1
    // Only 3 bytes follow — need 8 for entry header.
    blob.extend_from_slice(&[0u8, 0, 0]);
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("v2 tail truncated at entry header"),
        "expected v2-entry-header truncation err, got {err:?}"
    );
}

/// Header + v2 count=1, full entry header declaring tags_len=10,
/// but only 3 tag bytes provided.
#[test]
fn decode_rejects_v2_tail_truncated_at_entry_tags() {
    let mut blob = empty_header();
    blob.extend_from_slice(&1u32.to_le_bytes()); // count=1
    blob.extend_from_slice(&5u32.to_le_bytes()); // cont_pc=5
    blob.extend_from_slice(&10u32.to_le_bytes()); // tags_len=10
    blob.extend_from_slice(&[0u8, 0, 0]); // only 3 tag bytes
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("v2 tail truncated at entry tags"),
        "expected v2-entry-tags truncation err, got {err:?}"
    );
}

/// Header + valid (empty) v2 tail + 1 byte dangling where the v3
/// count u32 should start.
#[test]
fn decode_rejects_v3_tail_truncated_at_count() {
    let mut blob = empty_header();
    blob.extend_from_slice(&0u32.to_le_bytes()); // v2 count=0
    blob.push(0); // 1 byte where v3 count (u32) needed
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("v3 tail truncated at count"),
        "expected v3-count truncation err, got {err:?}"
    );
}

/// Header + empty v2 + v3 count=1 but per-entry header (12 bytes
/// = cont_pc+head_resume_pc+tags_len) is short.
#[test]
fn decode_rejects_v3_tail_truncated_at_entry_header() {
    let mut blob = empty_header();
    blob.extend_from_slice(&0u32.to_le_bytes()); // v2 count=0
    blob.extend_from_slice(&1u32.to_le_bytes()); // v3 count=1
    // Only 5 bytes follow — need 12 for entry header.
    blob.extend_from_slice(&[0u8; 5]);
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("v3 tail truncated at entry header"),
        "expected v3-entry-header truncation err, got {err:?}"
    );
}

/// Header + empty v2 + v3 entry with tags_len=8 but only 2 tag
/// bytes.
#[test]
fn decode_rejects_v3_tail_truncated_at_entry_tags() {
    let mut blob = empty_header();
    blob.extend_from_slice(&0u32.to_le_bytes()); // v2 count=0
    blob.extend_from_slice(&1u32.to_le_bytes()); // v3 count=1
    blob.extend_from_slice(&3u32.to_le_bytes()); // cont_pc
    blob.extend_from_slice(&4u32.to_le_bytes()); // head_resume_pc
    blob.extend_from_slice(&8u32.to_le_bytes()); // tags_len=8
    blob.extend_from_slice(&[0u8, 0]); // only 2 tag bytes
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("v3 tail truncated at entry tags"),
        "expected v3-entry-tags truncation err, got {err:?}"
    );
}

/// Header + empty v2 + v3 entry with tags written but the
/// chain_bytes_len u32 itself is truncated.
#[test]
fn decode_rejects_v3_tail_truncated_at_chain_header() {
    let mut blob = empty_header();
    blob.extend_from_slice(&0u32.to_le_bytes()); // v2 count=0
    blob.extend_from_slice(&1u32.to_le_bytes()); // v3 count=1
    blob.extend_from_slice(&3u32.to_le_bytes()); // cont_pc
    blob.extend_from_slice(&4u32.to_le_bytes()); // head_resume_pc
    blob.extend_from_slice(&0u32.to_le_bytes()); // tags_len=0
    // No chain_bytes_len u32 follows — should fail "chain header"
    // truncation.
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("v3 tail truncated at chain header"),
        "expected v3-chain-header truncation err, got {err:?}"
    );
}

/// Header + empty v2 + v3 entry with tags + chain_bytes_len=24
/// declared but only 6 bytes provided.
#[test]
fn decode_rejects_v3_tail_truncated_at_chain_bytes() {
    let mut blob = empty_header();
    blob.extend_from_slice(&0u32.to_le_bytes()); // v2 count=0
    blob.extend_from_slice(&1u32.to_le_bytes()); // v3 count=1
    blob.extend_from_slice(&3u32.to_le_bytes()); // cont_pc
    blob.extend_from_slice(&4u32.to_le_bytes()); // head_resume_pc
    blob.extend_from_slice(&0u32.to_le_bytes()); // tags_len=0
    blob.extend_from_slice(&24u32.to_le_bytes()); // chain_bytes_len=24
    blob.extend_from_slice(&[0u8; 6]); // only 6 chain bytes
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("v3 tail truncated at chain bytes"),
        "expected v3-chain-bytes truncation err, got {err:?}"
    );
}

/// Negative-control sanity check: an empty-but-well-formed v3
/// blob (header + v2 count=0 + v3 count=0) must decode cleanly.
/// Catches the case where adding stricter checks accidentally
/// rejects the legit minimal shape.
#[test]
fn decode_accepts_minimal_well_formed_blob() {
    let mut blob = empty_header();
    blob.extend_from_slice(&0u32.to_le_bytes()); // v2 count=0
    blob.extend_from_slice(&0u32.to_le_bytes()); // v3 count=0
    let decoded = decode_meta_blob(&blob).expect("minimal blob decodes");
    assert!(decoded.entry_tags.is_empty());
    assert!(decoded.exit_tags.is_empty());
    assert!(decoded.per_exit_tags.is_empty());
    assert!(decoded.per_exit_inline.is_empty());
}
