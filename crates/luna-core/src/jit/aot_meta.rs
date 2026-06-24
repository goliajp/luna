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
//! # Wire format versions
//!
//! - **v1** — sub-piece-4 minimal cut. Fields: `head_pc`, `n_ops`,
//!   `window_size`, `dispatchable`, `entry_tags`, `exit_tags`,
//!   `global_tag_res_kind`. Only "simple" traces (no inline side-
//!   exits, no per-cont_pc tag exits) installable.
//! - **v2** — Stage 7 trace-coverage follow-up. Appends a trailing
//!   `per_exit_tags` array (`(cont_pc, [ExitTag])` per entry) so
//!   traces with typed-register side-exit guards (GetUpval-heavy
//!   closures, type-specialized GetField loops) are AOT-installable.
//!   v2 readers MUST accept v1 blobs as if `per_exit_tags`
//!   were empty (the trailing block becomes optional via the
//!   `total_payload < bytes.len()` predicate at decode time).
//! - **v3** — Inline cmp@d>0 side-exit scaffolding. Appends a second
//!   trailing block after `per_exit_tags`: a list of
//!   [`PerExitInlineEntry`] records carrying the `cont_pc`,
//!   `head_resume_pc`, the per-slot exit-tag snapshot covering the
//!   trace's full `window_size`, and the `FrameMaterializeInfo`
//!   chain bytes. v3 readers accept v1 and v2 blobs as if the
//!   inline block were empty.
//!
//!   **Important — install-time invariant**: emitting the v3 inline
//!   block into a meta blob is necessary but **not sufficient** to
//!   safely AOT-install a trace whose `per_exit_inline` is non-
//!   empty. The trace mcode itself today bakes a raw
//!   `Rc::as_ptr(&chain_rc)` value as an `iconst` immediate at lower
//!   time (see `luna-jit/src/jit_backend/trace.rs` near the cmp@d>0
//!   side-exit emit sites). Under JIT the immediate is a live heap
//!   pointer; under AOT it would be the warmup VM's heap address,
//!   invalid in the deploy binary. To actually unlock the install
//!   path the lowerer must:
//!     1. Emit a writable per-site slot (`__luna_aot_inline_chain_
//!        slot_<key>`) and load through it instead of the raw
//!        `iconst`.
//!     2. Register a new `luna_inline_chain_idx` bracketed section
//!        analogous to `luna_strkey_idx` so the deploy resolver can
//!        match installed entries to slot addresses.
//!     3. The deploy walker (after rebuilding `Rc<[FrameMaterializeInfo
//!        ]>` from v3 blob bytes) writes the live address of the
//!        rebuilt chain into each slot before the first dispatch.
//!
//!   Until those three pieces land, the AOT harvester
//!   (`luna-aot::embed`) MUST keep its
//!   `per_exit_inline.is_empty()` filter so non-empty-inline
//!   traces never produce a v3 blob with inline data — they stay
//!   JIT-only. The wire format below is forward-ready so the
//!   lowerer / resolver work doesn't need a fresh version bump.
//!
//! # Field summary
//!
//! `CompiledTrace` carries 30+ fields including `RefCell<HashMap>`,
//! `Box<Cell<*const u8>>`, `Rc<[InlineSideExit]>` — most are side-
//! trace bookkeeping irrelevant for AOT (the deploy `Vm` never side-
//! traces an AOT-installed trace, so all those start empty). The
//! AOT meta format serializes only the **dispatch-load-bearing**
//! fields:
//!
//! - `head_pc`, `n_ops`, `window_size`, `dispatchable`
//! - `entry_tags: Rc<[u8]>` — per-slot entry-tag specialization
//! - `exit_tags: Rc<[ExitTag]>` — per-slot exit-tag restore (clean tail)
//! - `global_tag_res_kind` — fast-path classification
//! - `per_exit_tags` *(v2+)* — per-cont_pc slot-shape entries the
//!   dispatcher uses to restore vm.stack on a typed-register
//!   side-exit
//! - `per_exit_inline` *(v3+)* — per-site inline cmp@d>0 side-exit
//!   records: cont_pc + head_resume_pc + per-slot exit_tags + the
//!   `FrameMaterializeInfo` chain bytes. Forward-compat scaffold;
//!   the harvester does not yet emit non-empty entries because the
//!   trace mcode side needs a relocatable chain-slot scheme first
//!   (see the v3 paragraph above).
//!
//! `body_writes`, side-trace ptrs etc. default to empty / null.

use crate::jit::trace_types::{ExitTag, FrameMaterializeInfo, TagResKind};

/// Magic bytes at the start of every AOT meta blob. The deploy walker
/// checks this against `read::<u32>` before parsing the rest;
/// mismatches are reported (and the entry skipped) rather than
/// causing arbitrary deserialization.
pub const AOT_META_MAGIC: u32 = 0xAA77_0001;

/// Wire-format version. v1 = minimal cut (sub-piece 4). v2 = appends
/// trailing `per_exit_tags` block so typed-register side-exits
/// (GetUpval-heavy traces) install at deploy time. v3 = appends a
/// second trailing block carrying `per_exit_inline` data so
/// depth>0 inlined cmp side-exits can be rebuilt at install time
/// (the trace mcode side still needs a relocatable-slot scheme
/// before non-empty inline entries can actually be emitted —
/// module docs go into the staging plan).
///
/// **Forward compatibility contract**: a v3 writer emits the same
/// fixed-prefix header layout as v1/v2 plus the v2 tail (always
/// present, count=0 if empty) plus the v3 tail (count=0 if empty).
/// A v3 reader MUST accept v1 blobs (= header + tags only, no v2
/// tail) and v2 blobs (= header + tags + v2 tail only, no v3 tail)
/// as if the missing tails were empty — implementation lives in
/// [`decode_meta_blob`]'s `bytes.len() > cur` predicate at each
/// tail boundary. v2 readers on a v3 blob would mis-parse the v3
/// tail as garbage; we bump `AOT_META_VERSION` so older readers
/// hard-reject instead of silently mis-installing.
pub const AOT_META_VERSION: u32 = 3;

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

/// One per-cont_pc side-exit entry serialized into the v2 tail of a
/// meta blob. Mirrors `CompiledTrace::per_exit_tags`'s `(u32,
/// Rc<[ExitTag]>)` shape, with the `ExitTag` slice already packed
/// through [`pack_exit_tag`].
#[derive(Clone, Debug)]
pub struct PerExitTagsEntry {
    /// Pc the interp resumes at after the side-exit fires. Matches
    /// the IR's `iconst` baked into the side-exit return.
    pub cont_pc: u32,
    /// Per-slot `ExitTag` snapshot at the side-exit moment, packed
    /// via [`pack_exit_tag`]. Length is the trace's caller-window
    /// `max_stack` (always ≤ `window_size`).
    pub tags_packed: Vec<u8>,
}

/// One per-site inline cmp@d>0 side-exit entry serialized into the v3
/// tail of a meta blob. Mirrors `CompiledTrace::per_exit_inline`'s
/// [`crate::jit::trace_types::InlineSideExit`] shape minus the
/// runtime-only `side_trace_ptr` cell (always defaults null on AOT
/// install). The `chain` field carries [`FrameMaterializeInfo`]
/// records as raw bytes — `FrameMaterializeInfo` is `repr(C)` with
/// three 32-bit fields = exactly 12 bytes per entry on every
/// supported target, so the wire layout is stable.
#[derive(Clone, Debug)]
pub struct PerExitInlineEntry {
    /// Pc the interpreter resumes at after the inline side-exit
    /// fires. Mirrors `InlineSideExit::cont_pc`.
    pub cont_pc: u32,
    /// Pc to write on the trace head frame when the side-exit fires
    /// (the outermost self-rec Call's `pc + 1`). Mirrors
    /// `InlineSideExit::head_resume_pc`.
    pub head_resume_pc: u32,
    /// Per-slot `ExitTag` snapshot at the side-exit moment, packed
    /// via [`pack_exit_tag`]. Length equals the trace's full
    /// `window_size` (caller + every inlined frame's register
    /// window) — `per_exit_tags`'s arrays cover only `max_stack`,
    /// inline arrays cover the full window.
    pub tags_packed: Vec<u8>,
    /// `FrameMaterializeInfo` records as raw bytes (count * 12).
    /// Outermost = depth 1 first, innermost = depth N last; the
    /// innermost frame's `pc` is already overwritten to the side-
    /// exit PC at AOT-compile time (matches the JIT-side
    /// snapshot.last_mut().pc = side_exit_pc step). Length divisible
    /// by 12 is a wire-format invariant; the decoder rejects otherwise.
    pub chain_bytes: Vec<u8>,
}

impl PerExitInlineEntry {
    /// Byte size of one `FrameMaterializeInfo` on the wire. Asserted
    /// at compile time via [`FRAME_MATERIALIZE_INFO_WIRE_SIZE_CHECK`]
    /// against the live struct so layout drift fails the build.
    pub const FRAME_MATERIALIZE_INFO_SIZE: usize = 12;

    /// Construct a `PerExitInlineEntry` from a live
    /// [`crate::jit::trace_types::InlineSideExit`]. The chain is
    /// serialized via a raw-byte copy — `FrameMaterializeInfo` is
    /// `repr(C)` + all-`Copy` fields with no padding, so the byte
    /// pattern matches the on-disk wire layout verbatim.
    ///
    /// # Safety
    ///
    /// `FrameMaterializeInfo` is `#[repr(C)]` with three 32-bit
    /// fields and no padding; the byte-level transmute below is
    /// sound per the wire-size assertion at module top.
    pub fn from_inline_side_exit(src: &crate::jit::trace_types::InlineSideExit) -> Self {
        let tags_packed: Vec<u8> = src.exit_tags.iter().copied().map(pack_exit_tag).collect();
        let n = src.chain.len();
        let mut chain_bytes = Vec::with_capacity(n * Self::FRAME_MATERIALIZE_INFO_SIZE);
        for fm in src.chain.iter() {
            chain_bytes.extend_from_slice(&fm.base_offset.to_le_bytes());
            chain_bytes.extend_from_slice(&fm.pc.to_le_bytes());
            chain_bytes.extend_from_slice(&fm.nresults.to_le_bytes());
        }
        PerExitInlineEntry {
            cont_pc: src.cont_pc,
            head_resume_pc: src.head_resume_pc,
            tags_packed,
            chain_bytes,
        }
    }

    /// Reconstruct a `Vec<FrameMaterializeInfo>` from this entry's
    /// `chain_bytes`. Returns `None` if `chain_bytes.len()` is not a
    /// multiple of [`Self::FRAME_MATERIALIZE_INFO_SIZE`] (corruption
    /// signal — the deploy walker should skip the entry).
    pub fn rebuild_chain(&self) -> Option<Vec<FrameMaterializeInfo>> {
        let unit = Self::FRAME_MATERIALIZE_INFO_SIZE;
        if !self.chain_bytes.len().is_multiple_of(unit) {
            return None;
        }
        let n = self.chain_bytes.len() / unit;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let off = i * unit;
            let base_offset =
                u32::from_le_bytes(self.chain_bytes[off..off + 4].try_into().unwrap());
            let pc = u32::from_le_bytes(self.chain_bytes[off + 4..off + 8].try_into().unwrap());
            let nresults =
                i32::from_le_bytes(self.chain_bytes[off + 8..off + 12].try_into().unwrap());
            out.push(FrameMaterializeInfo {
                base_offset,
                pc,
                nresults,
            });
        }
        Some(out)
    }
}

/// Static assertion that [`FrameMaterializeInfo`]'s in-memory size
/// matches the wire-format constant. A regression here (a fourth
/// field, a different layout attribute) silently misaligns the
/// `chain_bytes` (re)serialization; the build break makes the drift
/// loud at the source instead of mysterious at deploy time.
pub const FRAME_MATERIALIZE_INFO_WIRE_SIZE_CHECK: () = assert!(
    core::mem::size_of::<FrameMaterializeInfo>() == PerExitInlineEntry::FRAME_MATERIALIZE_INFO_SIZE,
    "FrameMaterializeInfo wire size drifted — update PerExitInlineEntry::FRAME_MATERIALIZE_INFO_SIZE \
     and any deploy-side rebuilders together"
);

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
        per_exit_tags.reserve(count);
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
        per_exit_inline.reserve(count);
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
    /// Padding so the following 64-bit address fields align at 8 bytes.
    pub _pad: u32,
    /// Address of the AOT-emitted trace fn
    /// (`extern "C" fn(*mut i64) -> i64`). Stored as `u64` so the
    /// wire layout is identical across 32/64-bit targets — wasm32 +
    /// other 32-bit targets cast through this field. AOT-binary
    /// deploy is always 64-bit (cross-compile to 32-bit targets
    /// disabled at the linker step), so the upper 32 bits are zero
    /// in practice. Linker-resolved relocation against the
    /// `luna_aot_trace_<idx>` symbol the lowerer exports.
    pub fn_ptr: u64,
    /// Address of the matching meta blob in `luna_trace_blob`. Same
    /// width-stable rationale as `fn_ptr`.
    pub meta_ptr: u64,
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
                // 2 frames = 24 bytes.
                chain_bytes: {
                    let mut v = Vec::new();
                    for (off, pc, nr) in [(2u32, 7u32, 1i32), (5u32, 11u32, 2i32)] {
                        v.extend_from_slice(&off.to_le_bytes());
                        v.extend_from_slice(&pc.to_le_bytes());
                        v.extend_from_slice(&nr.to_le_bytes());
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
        }];
        let live = InlineSideExit {
            cont_pc: 42,
            head_resume_pc: 50,
            exit_tags: std::rc::Rc::from(
                vec![ExitTag::Int, ExitTag::Float, ExitTag::Untouched].into_boxed_slice(),
            ),
            chain: std::rc::Rc::from(chain.into_boxed_slice()),
            side_trace_ptr: Box::new(std::cell::Cell::new(std::ptr::null())),
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
}
