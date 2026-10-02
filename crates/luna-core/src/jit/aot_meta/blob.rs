//! Encoding and decoding of one trace's meta blob.

use super::*;

/// Serialize a header + the two tag arrays + the v2 `per_exit_tags`
/// tail + the v3 `per_exit_inline` tail into a fresh `Vec<u8>`. Pass
/// empty slices to emit a "simple" trace (each tail then carries a
/// single `count = 0` u32 — still v3 layout, just empty).
///
/// The produced bytes are what `luna-aot` embeds into the
/// `luna_trace_blob` section per-trace; the deploy walker reads from
/// the same wire shape via [`decode_meta_blob`].
pub fn encode_meta_blob(
    header: &AotTraceMetaHeader,
    entry_tags: &[u8],
    exit_tags_packed: &[u8],
    per_exit_tags: &[PerExitTagsEntry],
    per_exit_inline: &[PerExitInlineEntry],
) -> Vec<u8> {
    assert_eq!(entry_tags.len(), header.entry_tags_len as usize);
    assert_eq!(exit_tags_packed.len(), header.exit_tags_len as usize);
    assert_eq!(header.version, AOT_META_VERSION);
    let v2_tail_bytes: usize = 4 + per_exit_tags
        .iter()
        .map(|e| 4 + 4 + e.tags_packed.len())
        .sum::<usize>();
    let v3_tail_bytes: usize = 4 + per_exit_inline
        .iter()
        .map(|e| 4 + 4 + 4 + e.tags_packed.len() + 4 + e.chain_bytes.len())
        .sum::<usize>();
    let mut out = Vec::with_capacity(
        AotTraceMetaHeader::SIZE
            + entry_tags.len()
            + exit_tags_packed.len()
            + v2_tail_bytes
            + v3_tail_bytes,
    );
    out.extend_from_slice(&header.magic.to_le_bytes());
    out.extend_from_slice(&header.version.to_le_bytes());
    out.extend_from_slice(&header.head_pc.to_le_bytes());
    out.extend_from_slice(&header.n_ops.to_le_bytes());
    out.extend_from_slice(&header.window_size.to_le_bytes());
    out.push(header.dispatchable);
    out.push(header.tag_res_kind);
    out.extend_from_slice(&header.entry_tags_len.to_le_bytes());
    out.extend_from_slice(&header.exit_tags_len.to_le_bytes());
    out.extend_from_slice(entry_tags);
    out.extend_from_slice(exit_tags_packed);
    // v2 tail: u32 count, then per entry [cont_pc:u32, tags_len:u32, tags:[u8; tags_len]].
    out.extend_from_slice(&(per_exit_tags.len() as u32).to_le_bytes());
    for ent in per_exit_tags {
        out.extend_from_slice(&ent.cont_pc.to_le_bytes());
        out.extend_from_slice(&(ent.tags_packed.len() as u32).to_le_bytes());
        out.extend_from_slice(&ent.tags_packed);
    }
    // v3 tail: u32 count, then per entry
    //   [cont_pc:u32, head_resume_pc:u32, tags_len:u32, tags:[u8; tags_len],
    //    chain_bytes_len:u32, chain_bytes:[u8; chain_bytes_len]].
    // chain_bytes_len is always a multiple of 12 (FrameMaterializeInfo
    // wire size); the decoder rejects otherwise as a corruption signal.
    out.extend_from_slice(&(per_exit_inline.len() as u32).to_le_bytes());
    for ent in per_exit_inline {
        out.extend_from_slice(&ent.cont_pc.to_le_bytes());
        out.extend_from_slice(&ent.head_resume_pc.to_le_bytes());
        out.extend_from_slice(&(ent.tags_packed.len() as u32).to_le_bytes());
        out.extend_from_slice(&ent.tags_packed);
        out.extend_from_slice(&(ent.chain_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&ent.chain_bytes);
    }
    out
}

/// Decoded shape returned by [`decode_meta_blob`].
#[derive(Debug)]
pub struct DecodedMeta {
    /// The fixed-prefix header.
    pub header: AotTraceMetaHeader,
    /// `entry_tags` payload (length = `header.entry_tags_len`).
    pub entry_tags: Vec<u8>,
    /// `exit_tags` payload (length = `header.exit_tags_len`), still in
    /// packed `u8` form. Caller maps each through [`unpack_exit_tag`].
    pub exit_tags: Vec<u8>,
    /// v2 tail — per-cont_pc tag arrays. Empty for v1 blobs and for
    /// v2+ traces with no typed-register side-exits.
    pub per_exit_tags: Vec<PerExitTagsEntry>,
    /// v3 tail — per-site inline cmp@d>0 side-exit metadata.
    /// Empty for v1 / v2 blobs and for v3 traces with no inlined
    /// side-exits. Today the AOT harvester filters out traces with
    /// non-empty `per_exit_inline` regardless (see module docs);
    /// the field exists so the wire format is forward-ready for the
    /// relocatable-chain-slot lowerer work that flips the filter.
    pub per_exit_inline: Vec<PerExitInlineEntry>,
}

/// Deserialize a blob produced by [`encode_meta_blob`]. Returns
/// `Err(reason)` on magic / version / length mismatch — the deploy
/// walker should skip the entry and log the reason rather than
/// installing a broken trace.
pub fn decode_meta_blob(bytes: &[u8]) -> Result<DecodedMeta, &'static str> {
    if bytes.len() < AotTraceMetaHeader::SIZE {
        return Err("blob shorter than header");
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    if magic != AOT_META_MAGIC {
        return Err("AOT_META_MAGIC mismatch");
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != AOT_META_VERSION {
        return Err("AOT_META_VERSION mismatch");
    }
    let head_pc = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let n_ops = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    let window_size = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
    let dispatchable = bytes[20];
    let tag_res_kind = bytes[21];
    let entry_tags_len = u16::from_le_bytes(bytes[22..24].try_into().unwrap());
    let exit_tags_len = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let header = AotTraceMetaHeader {
        magic,
        version,
        head_pc,
        n_ops,
        window_size,
        dispatchable,
        tag_res_kind,
        entry_tags_len,
        exit_tags_len,
    };
    let total_payload = entry_tags_len as usize + exit_tags_len as usize;
    if bytes.len() < AotTraceMetaHeader::SIZE + total_payload {
        return Err("blob shorter than declared payload");
    }
    let entry_start = AotTraceMetaHeader::SIZE;
    let entry_end = entry_start + entry_tags_len as usize;
    let exit_end = entry_end + exit_tags_len as usize;
    let entry_tags = bytes[entry_start..entry_end].to_vec();
    let exit_tags = bytes[entry_end..exit_end].to_vec();
    // v2 tail: optional per_exit_tags block. Absent (= empty) when
    // the blob ends exactly at `exit_end` — covers v1-shaped
    // producers (which never wrote a tail) and v2+ producers
    // serializing a trace with zero typed-register side-exits.
    let mut per_exit_tags: Vec<PerExitTagsEntry> = Vec::new();
    let mut cur = exit_end;
    if bytes.len() > cur {
        if bytes.len() < cur + 4 {
            return Err("v2 tail truncated at count");
        }
        let count = u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap()) as usize;
        cur += 4;
        // the count comes from the blob: reserve only what the rest of it
        // can hold (an entry takes at least its 8-byte header), so a
        // corrupt count fails in the loop below instead of asking for
        // gigabytes
        per_exit_tags.reserve(count.min((bytes.len() - cur) / 8));
        for _ in 0..count {
            if bytes.len() < cur + 8 {
                return Err("v2 tail truncated at entry header");
            }
            let cont_pc = u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap());
            cur += 4;
            let tags_len = u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap()) as usize;
            cur += 4;
            if bytes.len() < cur + tags_len {
                return Err("v2 tail truncated at entry tags");
            }
            let tags_packed = bytes[cur..cur + tags_len].to_vec();
            cur += tags_len;
            per_exit_tags.push(PerExitTagsEntry {
                cont_pc,
                tags_packed,
            });
        }
    }
    // v3 tail: optional per_exit_inline block. Absent when the blob
    // ends at the v2 tail boundary — covers v1 / v2 producers and
    // v3 producers serializing a trace with zero inline cmp@d>0
    // side-exits. Tail layout per entry:
    //   cont_pc:u32, head_resume_pc:u32,
    //   tags_len:u32, tags:[u8; tags_len],
    //   chain_bytes_len:u32, chain_bytes:[u8; chain_bytes_len]
    // chain_bytes_len validated as a multiple of 12
    // (FrameMaterializeInfo wire size) — non-multiple = corruption,
    // we Err so the deploy walker skips the entry cleanly.
    let mut per_exit_inline: Vec<PerExitInlineEntry> = Vec::new();
    if bytes.len() > cur {
        if bytes.len() < cur + 4 {
            return Err("v3 tail truncated at count");
        }
        let count = u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap()) as usize;
        cur += 4;
        // as above: an entry is at least 16 bytes (cont_pc, resume pc and
        // the two length fields)
        per_exit_inline.reserve(count.min((bytes.len() - cur) / 16));
        for _ in 0..count {
            if bytes.len() < cur + 12 {
                return Err("v3 tail truncated at entry header");
            }
            let cont_pc = u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap());
            cur += 4;
            let head_resume_pc = u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap());
            cur += 4;
            let tags_len = u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap()) as usize;
            cur += 4;
            if bytes.len() < cur + tags_len {
                return Err("v3 tail truncated at entry tags");
            }
            let tags_packed = bytes[cur..cur + tags_len].to_vec();
            cur += tags_len;
            if bytes.len() < cur + 4 {
                return Err("v3 tail truncated at chain header");
            }
            let chain_bytes_len =
                u32::from_le_bytes(bytes[cur..cur + 4].try_into().unwrap()) as usize;
            cur += 4;
            if bytes.len() < cur + chain_bytes_len {
                return Err("v3 tail truncated at chain bytes");
            }
            if !chain_bytes_len.is_multiple_of(PerExitInlineEntry::FRAME_MATERIALIZE_INFO_SIZE) {
                return Err("v3 tail chain_bytes_len not a multiple of FrameMaterializeInfo size");
            }
            let chain_bytes = bytes[cur..cur + chain_bytes_len].to_vec();
            cur += chain_bytes_len;
            per_exit_inline.push(PerExitInlineEntry {
                cont_pc,
                head_resume_pc,
                tags_packed,
                chain_bytes,
            });
        }
    }
    Ok(DecodedMeta {
        header,
        entry_tags,
        exit_tags,
        per_exit_tags,
        per_exit_inline,
    })
}
