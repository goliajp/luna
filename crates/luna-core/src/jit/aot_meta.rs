//! Wire format for AOT trace metadata.
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
//! Hand-rolled `u8` packing — no `bincode`, no `serde`. The header
//! carries [`AOT_META_MAGIC`] +
//! [`AOT_META_VERSION`]; a mismatch on the deploy side is a hard
//! reject, not silent fallback.
//!
//! # Wire format versions
//!
//! - **v1** — minimal format. Fields: `head_pc`, `n_ops`,
//!   `window_size`, `dispatchable`, `entry_tags`, `exit_tags`,
//!   `global_tag_res_kind`. Only "simple" traces (no inline side-
//!   exits, no per-cont_pc tag exits) installable.
//! - **v2** — Appends a trailing
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

mod blob;
pub use blob::*;

/// Magic bytes at the start of every AOT meta blob. The deploy walker
/// checks this against `read::<u32>` before parsing the rest;
/// mismatches are reported (and the entry skipped) rather than
/// causing arbitrary deserialization.
pub const AOT_META_MAGIC: u32 = 0xAA77_0001;

/// Wire-format version. v1 = minimal format. v2 = appends
/// trailing `per_exit_tags` block so typed-register side-exits
/// (GetUpval-heavy traces) install at deploy time. v3 = appends a
/// second trailing block carrying `per_exit_inline` data so
/// depth>0 inlined cmp side-exits can be rebuilt at install time
/// (the trace mcode side still needs a relocatable-slot scheme
/// before non-empty inline entries can actually be emitted —
/// see the module docs).
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
/// hard-reject instead of silently mis-installing. Version 4 keeps the
/// v3 layout with 16-byte chain records (`FrameMaterializeInfo` gained
/// `n_varargs`).
pub const AOT_META_VERSION: u32 = 4;

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
        ExitTag::Bool => 7,
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
        7 => Some(ExitTag::Bool),
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
    pub const FRAME_MATERIALIZE_INFO_SIZE: usize = 16;

    /// Construct a `PerExitInlineEntry` from a live
    /// [`crate::jit::trace_types::InlineSideExit`]. The chain is
    /// serialized via a raw-byte copy — `FrameMaterializeInfo` is
    /// `repr(C)` + all-`Copy` fields with no padding, so the byte
    /// pattern matches the on-disk wire layout verbatim.
    ///
    /// # Safety
    ///
    /// `FrameMaterializeInfo` is `#[repr(C)]` with four 32-bit
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
            chain_bytes.extend_from_slice(&fm.n_varargs.to_le_bytes());
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
            let n_varargs =
                u32::from_le_bytes(self.chain_bytes[off + 12..off + 16].try_into().unwrap());
            out.push(FrameMaterializeInfo {
                base_offset,
                pc,
                nresults,
                n_varargs,
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
mod tests;
