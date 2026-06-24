//! v1.3 Phase AOT Stage 7 sub-piece 4 — wire format for AOT trace
//! metadata.
//!
//! # Why a luna-core module
//!
//! The format is shared by two distinct crates:
//!
//! - `luna-aot` (compile-time): serializes a runtime `CompiledTrace`
//!   into bytes embedded in the AOT object's `luna_trace_blob`
//!   section.
//! - `luna-runtime-helpers` (deploy-time): walks the
//!   `luna_trace_meta` index section, deserializes each entry's blob
//!   into the **minimal fields** needed to construct a fresh
//!   `CompiledTrace` for the deploy `Vm`'s dispatcher.
//!
//! Putting the wire format under `luna-core` keeps both crates pinned
//! to the same constants without giving either a dep on the other.
//!
//! # 0-dep contract
//!
//! Hand-rolled `u8` packing — no `bincode`, no `serde`. Format is
//! stable across the v1.3 line (header carries [`AOT_META_MAGIC`] +
//! [`AOT_META_VERSION`]; a mismatch on the deploy side is a hard
//! reject, not silent fallback).
//!
//! # Why not full `CompiledTrace` serialization
//!
//! `CompiledTrace` carries 30+ fields including `RefCell<HashMap>`,
//! `Box<Cell<*const u8>>`, `Rc<[InlineSideExit]>` — most are
//! side-trace bookkeeping that's irrelevant for AOT (the deploy `Vm`
//! never side-traces an AOT-installed trace, so all those start
//! empty). The sub-piece-4 cut serializes only the **dispatch-load-
//! bearing** fields:
//!
//! - `head_pc`, `n_ops`, `window_size`, `dispatchable`
//! - `entry_tags: Rc<[u8]>` — per-slot entry-tag specialization
//! - `exit_tags: Rc<[ExitTag]>` — per-slot exit-tag restore
//! - `global_tag_res_kind` — fast-path classification
//!
//! Everything else defaults to its `..Default::default()` equivalent:
//! empty `per_exit_inline`, empty `per_exit_tags`, empty
//! `body_writes`, null side-trace ptrs, etc. Traces with non-trivial
//! side-exit shapes (`per_exit_inline.len() > 0` etc.) **are
//! intentionally not installable** through this format — the AOT-side
//! recorder must filter to traces whose `is_aot_installable()` returns
//! true.

use crate::jit::trace_types::{ExitTag, TagResKind};

/// Magic bytes at the start of every AOT meta blob. The deploy walker
/// checks this against `read::<u32>` before parsing the rest;
/// mismatches are reported (and the entry skipped) rather than
/// causing arbitrary deserialization.
pub const AOT_META_MAGIC: u32 = 0xAA77_0001;

/// Wire-format version. Incremented whenever a field is added /
/// removed / its on-disk shape changes. The deploy walker hard-rejects
/// any blob whose version != this crate's `AOT_META_VERSION` — same-
/// version invariant is the only way to keep the format independent of
/// `CompiledTrace`'s field churn.
pub const AOT_META_VERSION: u32 = 1;

/// Fixed-size header at the top of every meta blob. All ints are
/// little-endian.
///
/// Total = 28 bytes. The variable-length tag arrays follow this
/// header back-to-back (`entry_tags_len` u8s then `exit_tags_len`
/// u8s).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AotTraceMetaHeader {
    /// [`AOT_META_MAGIC`]. Deploy-side hard-rejects mismatch.
    pub magic: u32,
    /// [`AOT_META_VERSION`]. Deploy-side hard-rejects mismatch.
    pub version: u32,
    /// Trace's `head_pc` — the PC the dispatcher matches on.
    pub head_pc: u32,
    /// Trace's `n_ops` — diagnostic only on the deploy side.
    pub n_ops: u32,
    /// Trace's `window_size` — sizes the dispatcher's `reg_state` buffer.
    pub window_size: u32,
    /// Trace's `dispatchable` flag as `u8` (0 / 1).
    pub dispatchable: u8,
    /// Trace's `global_tag_res_kind` packed:
    /// `0 = AllUntouched`, `1 = AllInt`, `2 = Mixed`.
    pub tag_res_kind: u8,
    /// Length of the `entry_tags` array that follows the header.
    /// `u16` is enough: trace's `max_stack` is bounded by Lua's
    /// `MAXREGS` (255) and even worst-case inlining caps under 4K.
    pub entry_tags_len: u16,
    /// Length of the `exit_tags` array that follows after `entry_tags`.
    pub exit_tags_len: u32,
}

impl AotTraceMetaHeader {
    /// Byte size of the fixed prefix. Used to compute payload offset.
    pub const SIZE: usize = 28;
}

/// Pack an `ExitTag` into its on-disk `u8` representation. Mirrors the
/// `#[repr(u8)]` discriminant so the wire format is the same byte the
/// compiler would lay out — but we go through the explicit match so a
/// future reorder of [`ExitTag`]'s variants doesn't silently change
/// the format.
pub fn pack_exit_tag(t: ExitTag) -> u8 {
    match t {
        ExitTag::Untouched => 0,
        ExitTag::Int => 1,
        ExitTag::Float => 2,
        ExitTag::Table => 3,
        ExitTag::Closure => 4,
        ExitTag::Nil => 5,
        ExitTag::Str => 6,
    }
}

/// Inverse of [`pack_exit_tag`]. Returns `None` on an unknown byte
/// (treated as a corruption signal by the deploy walker).
pub fn unpack_exit_tag(b: u8) -> Option<ExitTag> {
    match b {
        0 => Some(ExitTag::Untouched),
        1 => Some(ExitTag::Int),
        2 => Some(ExitTag::Float),
        3 => Some(ExitTag::Table),
        4 => Some(ExitTag::Closure),
        5 => Some(ExitTag::Nil),
        6 => Some(ExitTag::Str),
        _ => None,
    }
}

/// Pack a [`TagResKind`] into its wire byte.
pub fn pack_tag_res_kind(k: TagResKind) -> u8 {
    match k {
        TagResKind::AllUntouched => 0,
        TagResKind::AllInt => 1,
        TagResKind::Mixed => 2,
    }
}

/// Inverse of [`pack_tag_res_kind`]. Returns `None` on an unknown byte.
pub fn unpack_tag_res_kind(b: u8) -> Option<TagResKind> {
    match b {
        0 => Some(TagResKind::AllUntouched),
        1 => Some(TagResKind::AllInt),
        2 => Some(TagResKind::Mixed),
        _ => None,
    }
}

/// Serialize a header + the two tag arrays into a fresh `Vec<u8>`. The
/// produced bytes are what `luna-aot` embeds into the
/// `luna_trace_blob` section per-trace; the deploy walker reads from
/// the same wire shape.
pub fn encode_meta_blob(
    header: &AotTraceMetaHeader,
    entry_tags: &[u8],
    exit_tags_packed: &[u8],
) -> Vec<u8> {
    assert_eq!(entry_tags.len(), header.entry_tags_len as usize);
    assert_eq!(exit_tags_packed.len(), header.exit_tags_len as usize);
    let mut out =
        Vec::with_capacity(AotTraceMetaHeader::SIZE + entry_tags.len() + exit_tags_packed.len());
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
    Ok(DecodedMeta {
        header,
        entry_tags,
        exit_tags,
    })
}

/// Index entry layout in the deploy-side `luna_trace_meta` section.
///
/// 48 bytes per entry; the static linker fills `fn_ptr` and `meta_ptr`
/// with relocations resolving to the trace's `.text` body and the
/// matching `luna_trace_blob` payload respectively.
///
/// The deploy walker brackets the section via linker-synthetic
/// `__start_luna_trace_meta` / `__stop_luna_trace_meta` (ELF) or
/// `section$start$__DATA$luna_trace_meta` (Mach-O), mirroring sub-
/// piece 3's `luna_strkey_idx` plumbing.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AotTraceIndexEntry {
    /// `Proto::stable_hash()` — matches the AOT-time proto identity
    /// against the deploy-loaded proto tree.
    pub proto_hash: [u8; 16],
    /// Trace's `head_pc`. Used together with `proto_hash` to detect
    /// duplicate installs and to log which trace fired.
    pub head_pc: u32,
    /// Padding so the following pointer aligns at 8 bytes on every
    /// supported target.
    pub _pad: u32,
    /// Address of the AOT-emitted trace fn (`extern "C" fn(*mut i64) -> i64`).
    /// Linker-resolved relocation against the `luna_aot_trace_<idx>`
    /// symbol the lowerer exports.
    pub fn_ptr: *const u8,
    /// Address of the matching meta blob in `luna_trace_blob`.
    pub meta_ptr: *const u8,
    /// Length of the meta blob (the deploy walker hard-rejects entries
    /// whose declared payload exceeds this).
    pub meta_len: u32,
    /// Padding so the entry is a multiple of 8 bytes (48 total).
    pub _pad2: u32,
}

impl AotTraceIndexEntry {
    /// Byte size of one index entry. Compile-time assertion lives
    /// next to the type via [`AOT_TRACE_INDEX_ENTRY_SIZE_CHECK`].
    pub const SIZE: usize = 48;
}

/// Static assertion that `AotTraceIndexEntry` is exactly 48 bytes on
/// the host build. Both crates that consume this format (`luna-aot`,
/// `luna-runtime-helpers`) inherit the assertion via the type, so a
/// padding regression fails compilation before the wire format
/// silently misaligns.
pub const AOT_TRACE_INDEX_ENTRY_SIZE_CHECK: () = assert!(
    core::mem::size_of::<AotTraceIndexEntry>() == AotTraceIndexEntry::SIZE,
    "AotTraceIndexEntry must be 48 bytes — alignment / padding regressed"
);

#[cfg(test)]
mod tests {
    use super::*;

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
        let blob = encode_meta_blob(&header, &entry_tags, &exit_tags);
        assert_eq!(blob.len(), AotTraceMetaHeader::SIZE + 2 + 3);
        let decoded = decode_meta_blob(&blob).expect("decode");
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
}
