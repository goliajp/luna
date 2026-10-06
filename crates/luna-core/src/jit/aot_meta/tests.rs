use super::*;

/// A tail count the rest of the blob cannot hold is a truncation. It
/// sized a `reserve` before, so a corrupt count asked for tens of
/// gigabytes and aborted the process (found by `fuzz_aot_meta`).
#[test]
fn a_tail_count_larger_than_the_blob_is_refused_before_allocating() {
    let header = AotTraceMetaHeader {
        magic: AOT_META_MAGIC,
        version: AOT_META_VERSION,
        head_pc: 0,
        n_ops: 0,
        window_size: 0,
        dispatchable: 1,
        tag_res_kind: pack_tag_res_kind(TagResKind::AllInt),
        entry_tags_len: 0,
        exit_tags_len: 0,
    };
    let blob = encode_meta_blob(&header, &[], &[], &[], &[]);
    let v2 = AotTraceMetaHeader::SIZE;
    let mut bad = blob.clone();
    bad[v2..v2 + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        decode_meta_blob(&bad).err(),
        Some("v2 tail truncated at entry header")
    );
    let mut bad = blob;
    bad[v2 + 4..v2 + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        decode_meta_blob(&bad).err(),
        Some("v3 tail truncated at entry header")
    );
}

#[test]
fn header_round_trip() {
    let header = AotTraceMetaHeader {
        magic: AOT_META_MAGIC,
        version: AOT_META_VERSION,
        head_pc: 42,
        n_ops: 7,
        window_size: 4,
        dispatchable: 1,
        tag_res_kind: pack_tag_res_kind(TagResKind::AllInt),
        entry_tags_len: 2,
        exit_tags_len: 3,
    };
    let entry_tags = vec![1u8, 2u8];
    let exit_tags = vec![
        pack_exit_tag(ExitTag::Int),
        pack_exit_tag(ExitTag::Untouched),
        pack_exit_tag(ExitTag::Float),
    ];
    let blob = encode_meta_blob(&header, &entry_tags, &exit_tags, &[], &[]);
    // SIZE + entry_tags + exit_tags + v2-tail-count(4) + v3-tail-count(4)
    assert_eq!(blob.len(), AotTraceMetaHeader::SIZE + 2 + 3 + 4 + 4);
    let decoded = decode_meta_blob(&blob).expect("decode");
    assert!(decoded.per_exit_tags.is_empty());
    assert!(decoded.per_exit_inline.is_empty());
    assert_eq!(decoded.header.head_pc, 42);
    assert_eq!(decoded.header.window_size, 4);
    assert_eq!(decoded.header.dispatchable, 1);
    assert_eq!(decoded.entry_tags, entry_tags);
    assert_eq!(decoded.exit_tags, exit_tags);
    assert_eq!(
        unpack_tag_res_kind(decoded.header.tag_res_kind),
        Some(TagResKind::AllInt)
    );
    for (raw, expected) in
        decoded
            .exit_tags
            .iter()
            .zip([ExitTag::Int, ExitTag::Untouched, ExitTag::Float])
    {
        assert_eq!(unpack_exit_tag(*raw), Some(expected));
    }
}

#[test]
fn decode_rejects_magic_mismatch() {
    let mut blob = vec![0u8; AotTraceMetaHeader::SIZE];
    // Magic stays zero.
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(err.contains("MAGIC"));
    // Now valid magic + wrong version.
    blob[..4].copy_from_slice(&AOT_META_MAGIC.to_le_bytes());
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(err.contains("VERSION"));
}

#[test]
fn v2_per_exit_tags_round_trip() {
    // Two entries — one shorter than the other so the tail walker
    // exercises variable-length parsing per entry.
    let header = AotTraceMetaHeader {
        magic: AOT_META_MAGIC,
        version: AOT_META_VERSION,
        head_pc: 7,
        n_ops: 12,
        window_size: 5,
        dispatchable: 1,
        tag_res_kind: pack_tag_res_kind(TagResKind::Mixed),
        entry_tags_len: 0,
        exit_tags_len: 0,
    };
    let entries = vec![
        PerExitTagsEntry {
            cont_pc: 3,
            tags_packed: vec![
                pack_exit_tag(ExitTag::Int),
                pack_exit_tag(ExitTag::Untouched),
            ],
        },
        PerExitTagsEntry {
            cont_pc: 11,
            tags_packed: vec![
                pack_exit_tag(ExitTag::Closure),
                pack_exit_tag(ExitTag::Table),
                pack_exit_tag(ExitTag::Float),
            ],
        },
    ];
    let blob = encode_meta_blob(&header, &[], &[], &entries, &[]);
    let decoded = decode_meta_blob(&blob).expect("decode v2");
    assert_eq!(decoded.per_exit_tags.len(), 2);
    assert_eq!(decoded.per_exit_tags[0].cont_pc, 3);
    assert_eq!(decoded.per_exit_tags[0].tags_packed.len(), 2);
    assert_eq!(decoded.per_exit_tags[1].cont_pc, 11);
    assert_eq!(decoded.per_exit_tags[1].tags_packed.len(), 3);
    assert!(decoded.per_exit_inline.is_empty());
}

#[test]
fn v3_per_exit_inline_round_trip() {
    // Two inline-side-exit entries with different chain depths so
    // the tail walker exercises variable-length per-entry parsing.
    let header = AotTraceMetaHeader {
        magic: AOT_META_MAGIC,
        version: AOT_META_VERSION,
        head_pc: 0,
        n_ops: 0,
        window_size: 8,
        dispatchable: 1,
        tag_res_kind: pack_tag_res_kind(TagResKind::Mixed),
        entry_tags_len: 0,
        exit_tags_len: 0,
    };
    // Hand-roll the inline entries (the live-CompiledTrace
    // converter is exercised by the round-trip-from-live test
    // below).
    let inline = vec![
        PerExitInlineEntry {
            cont_pc: 5,
            head_resume_pc: 9,
            tags_packed: vec![pack_exit_tag(ExitTag::Int), pack_exit_tag(ExitTag::Int)],
            // 1 FrameMaterializeInfo = 12 bytes:
            //   base_offset = 3, pc = 4, nresults = 1
            chain_bytes: {
                let mut v = Vec::new();
                v.extend_from_slice(&3u32.to_le_bytes());
                v.extend_from_slice(&4u32.to_le_bytes());
                v.extend_from_slice(&1i32.to_le_bytes());
                v.extend_from_slice(&0u32.to_le_bytes());
                v
            },
        },
        PerExitInlineEntry {
            cont_pc: 17,
            head_resume_pc: 21,
            tags_packed: vec![
                pack_exit_tag(ExitTag::Closure),
                pack_exit_tag(ExitTag::Untouched),
                pack_exit_tag(ExitTag::Float),
            ],
            // 2 frames = 32 bytes.
            chain_bytes: {
                let mut v = Vec::new();
                for (off, pc, nr, va) in [(2u32, 7u32, 1i32, 0u32), (5u32, 11u32, 2i32, 3u32)] {
                    v.extend_from_slice(&off.to_le_bytes());
                    v.extend_from_slice(&pc.to_le_bytes());
                    v.extend_from_slice(&nr.to_le_bytes());
                    v.extend_from_slice(&va.to_le_bytes());
                }
                v
            },
        },
    ];
    let blob = encode_meta_blob(&header, &[], &[], &[], &inline);
    let decoded = decode_meta_blob(&blob).expect("decode v3");
    assert_eq!(decoded.per_exit_inline.len(), 2);
    assert_eq!(decoded.per_exit_inline[0].cont_pc, 5);
    assert_eq!(decoded.per_exit_inline[0].head_resume_pc, 9);
    assert_eq!(decoded.per_exit_inline[0].tags_packed.len(), 2);
    let chain0 = decoded.per_exit_inline[0]
        .rebuild_chain()
        .expect("rebuild chain[0]");
    assert_eq!(chain0.len(), 1);
    assert_eq!(chain0[0].base_offset, 3);
    assert_eq!(chain0[0].pc, 4);
    assert_eq!(chain0[0].nresults, 1);
    let chain1 = decoded.per_exit_inline[1]
        .rebuild_chain()
        .expect("rebuild chain[1]");
    assert_eq!(chain1.len(), 2);
    assert_eq!(chain1[0].base_offset, 2);
    assert_eq!(chain1[1].pc, 11);
    assert_eq!(chain1[1].nresults, 2);
    assert_eq!(chain1[1].n_varargs, 3);
}

#[test]
fn v3_per_exit_inline_round_trip_from_live() {
    // Exercise PerExitInlineEntry::from_inline_side_exit against
    // a hand-built InlineSideExit, then encode + decode +
    // rebuild and check field equality. Catches drift if either
    // the live-struct shape or the wire layout changes without
    // updating the other.
    use crate::jit::trace_types::{ExitTag, FrameMaterializeInfo, InlineSideExit};
    let chain = vec![FrameMaterializeInfo {
        base_offset: 1,
        pc: 2,
        nresults: 3,
        n_varargs: 4,
    }];
    let live = InlineSideExit {
        cont_pc: 42,
        head_resume_pc: 50,
        exit_tags: crate::jit::send_compat::TArc::from(
            vec![ExitTag::Int, ExitTag::Float, ExitTag::Untouched].into_boxed_slice(),
        ),
        chain: crate::jit::send_compat::TArc::from(chain.into_boxed_slice()),
        side_trace_ptr: Box::new(crate::jit::send_compat::TCellPtr::null()),
    };
    let entry = PerExitInlineEntry::from_inline_side_exit(&live);
    assert_eq!(entry.cont_pc, 42);
    assert_eq!(entry.head_resume_pc, 50);
    assert_eq!(entry.tags_packed.len(), 3);
    assert_eq!(
        entry.chain_bytes.len(),
        PerExitInlineEntry::FRAME_MATERIALIZE_INFO_SIZE
    );
    let header = AotTraceMetaHeader {
        magic: AOT_META_MAGIC,
        version: AOT_META_VERSION,
        head_pc: 0,
        n_ops: 0,
        window_size: 4,
        dispatchable: 1,
        tag_res_kind: pack_tag_res_kind(TagResKind::Mixed),
        entry_tags_len: 0,
        exit_tags_len: 0,
    };
    let blob = encode_meta_blob(&header, &[], &[], &[], &[entry]);
    let decoded = decode_meta_blob(&blob).expect("decode v3 from live");
    assert_eq!(decoded.per_exit_inline.len(), 1);
    let rebuilt = decoded.per_exit_inline[0].rebuild_chain().expect("rebuild");
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(rebuilt[0].base_offset, 1);
    assert_eq!(rebuilt[0].pc, 2);
    assert_eq!(rebuilt[0].nresults, 3);
    assert_eq!(rebuilt[0].n_varargs, 4);
}

#[test]
fn decode_rejects_v3_chain_bytes_misaligned() {
    // Hand-emit a v3 blob whose chain_bytes_len is not a multiple
    // of 12 — the decoder MUST refuse (returning Err) instead of
    // silently truncating, so the deploy walker has a clean skip
    // signal.
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
    let mut blob = Vec::new();
    blob.extend_from_slice(&header.magic.to_le_bytes());
    blob.extend_from_slice(&header.version.to_le_bytes());
    blob.extend_from_slice(&header.head_pc.to_le_bytes());
    blob.extend_from_slice(&header.n_ops.to_le_bytes());
    blob.extend_from_slice(&header.window_size.to_le_bytes());
    blob.push(header.dispatchable);
    blob.push(header.tag_res_kind);
    blob.extend_from_slice(&header.entry_tags_len.to_le_bytes());
    blob.extend_from_slice(&header.exit_tags_len.to_le_bytes());
    // v2 tail: count=0.
    blob.extend_from_slice(&0u32.to_le_bytes());
    // v3 tail: count=1, cont_pc=0, head_resume_pc=0, tags_len=0,
    // chain_bytes_len=7 (not a multiple of 12), chain_bytes=7 zeros.
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    blob.extend_from_slice(&7u32.to_le_bytes());
    blob.extend(std::iter::repeat_n(0u8, 7));
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(
        err.contains("FrameMaterializeInfo"),
        "expected misalignment err, got {err:?}"
    );
}

#[test]
fn decode_tolerates_v1_blob_shape() {
    // Emulate a v1-shaped blob: header + tags, NO trailing v2/v3
    // tails. The v3 decoder should accept as empty everywhere.
    let header = AotTraceMetaHeader {
        magic: AOT_META_MAGIC,
        version: AOT_META_VERSION,
        head_pc: 0,
        n_ops: 0,
        window_size: 0,
        dispatchable: 0,
        tag_res_kind: 0,
        entry_tags_len: 1,
        exit_tags_len: 0,
    };
    let mut blob = Vec::new();
    blob.extend_from_slice(&header.magic.to_le_bytes());
    blob.extend_from_slice(&header.version.to_le_bytes());
    blob.extend_from_slice(&header.head_pc.to_le_bytes());
    blob.extend_from_slice(&header.n_ops.to_le_bytes());
    blob.extend_from_slice(&header.window_size.to_le_bytes());
    blob.push(header.dispatchable);
    blob.push(header.tag_res_kind);
    blob.extend_from_slice(&header.entry_tags_len.to_le_bytes());
    blob.extend_from_slice(&header.exit_tags_len.to_le_bytes());
    blob.push(0); // entry_tags[0]
    // No v2/v3 tails.
    let decoded = decode_meta_blob(&blob).expect("decode v1-shaped");
    assert!(decoded.per_exit_tags.is_empty());
    assert!(decoded.per_exit_inline.is_empty());
}

#[test]
fn decode_tolerates_v2_blob_shape() {
    // Emulate a v2-shaped blob: header + tags + v2 tail only,
    // NO trailing v3 count u32. The v3 decoder should accept it
    // as an empty per_exit_inline.
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
    let mut blob = Vec::new();
    blob.extend_from_slice(&header.magic.to_le_bytes());
    blob.extend_from_slice(&header.version.to_le_bytes());
    blob.extend_from_slice(&header.head_pc.to_le_bytes());
    blob.extend_from_slice(&header.n_ops.to_le_bytes());
    blob.extend_from_slice(&header.window_size.to_le_bytes());
    blob.push(header.dispatchable);
    blob.push(header.tag_res_kind);
    blob.extend_from_slice(&header.entry_tags_len.to_le_bytes());
    blob.extend_from_slice(&header.exit_tags_len.to_le_bytes());
    // v2 tail: count=1, one entry (cont_pc=3, tags_len=1, tags=[Int])
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&3u32.to_le_bytes());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.push(pack_exit_tag(ExitTag::Int));
    // No v3 tail.
    let decoded = decode_meta_blob(&blob).expect("decode v2-shaped");
    assert_eq!(decoded.per_exit_tags.len(), 1);
    assert!(decoded.per_exit_inline.is_empty());
}

#[test]
fn decode_rejects_truncated() {
    // Header is fine, but exit_tags_len declares 10 bytes that
    // aren't there.
    let header = AotTraceMetaHeader {
        magic: AOT_META_MAGIC,
        version: AOT_META_VERSION,
        head_pc: 0,
        n_ops: 0,
        window_size: 0,
        dispatchable: 0,
        tag_res_kind: 0,
        entry_tags_len: 0,
        exit_tags_len: 10,
    };
    let blob = {
        let mut b = Vec::new();
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
    };
    // Only header, no payload — should fail truncation check.
    let err = decode_meta_blob(&blob).unwrap_err();
    assert!(err.contains("payload"));
}
