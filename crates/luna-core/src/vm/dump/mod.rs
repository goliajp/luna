//! Binary-chunk dump / undump entry point.
//!
//! Phase LB Wave 1 (v1.3): refactored from the single 380-LOC `dump.rs`
//! into a directory module to give Wave 2's five per-dialect PUC
//! translators a stable surface to land in parallel without stepping on
//! one another.
//!
//! Sub-modules:
//! - `luna` — luna's own dump / undump (per-dialect PUC header + a
//!   luna-specific body sentinel-tagged `"\x00LunaV1\x00"`).
//! - `reader` — shared byte-stream reader + PUC `loadSize` ULEB128 port
//!   (0-dep — luna-core contract forbids `leb128` / `byteorder` crates).
//! - `puc` — magic-byte → per-dialect PUC undumper dispatch. Wave 1
//!   ships stubs that return `Err("… not yet implemented (Phase LBN)")`
//!   for each of `5.{1,2,3,4,5}`; Wave 2 fills them in.
//!
//! Public surface (re-exported here so the 6 call sites — `builtins.rs`,
//! `exec.rs`, `lib_os_io.rs`, `lib_string.rs` — keep compiling):
//! - [`dump`] — `Proto → Vec<u8>` (luna body format)
//! - [`undump`] — bytes → `Gc<Proto>`, routes by leading magic byte
//! - [`is_binary_chunk`] — true for any `\x1b`-prefixed input (matches
//!   both luna and PUC bodies; loader uses this to decide
//!   "undump vs parse")

mod luna;
mod puc;
mod reader;
mod verify;

use crate::runtime::function::Proto;
use crate::runtime::heap::{Gc, Heap};
use crate::version::LuaVersion;

/// Serialise a function prototype to a binary chunk.
///
/// Delegates to `luna::dump` (private sibling module); output is luna's
/// own body format (PUC dialect header + `"\x00LunaV1\x00"` sentinel +
/// luna body). Not PUC-loadable — see RFC v1.3 §"open questions"-4 for
/// the PUC-output `string.dump` v1.4 candidate.
pub fn dump(proto: &Proto, strip: bool, version: LuaVersion) -> Vec<u8> {
    luna::dump(proto, strip, version)
}

/// True when `bytes` is a binary chunk (luna or PUC) — only the escape
/// byte is needed to disambiguate from source. Matches PUC's
/// `lua_load`-side "starts with `\x1b`?" check.
pub fn is_binary_chunk(bytes: &[u8]) -> bool {
    bytes.first() == Some(&0x1b)
}

/// Reconstruct a prototype tree from a binary chunk.
///
/// luna's own chunks carry the running dialect's PUC header followed by the
/// `BODY_TAG` sentinel; a PUC chunk has its body there instead. Routing:
/// - `BODY_TAG` right after a header-sized prefix → `luna::undump`. The
///   header bytes are not checked here, so a luna chunk with a corrupted
///   header reaches luna's own "bad header" errors (calls.lua pins them).
/// - shorter than header + tag and not foreign → `luna::undump`, which
///   reports the truncation.
/// - otherwise a `\x1bLua` chunk with a `0x51..0x55` version byte is PUC's
///   → `puc::undump_puc`, gated by `allow_puc`. This includes a chunk from
///   the running dialect's own PUC version, which routing by version byte
///   alone used to send to luna's loader.
/// - anything else → `luna::undump` for its error.
///
/// `allow_puc` mirrors `Vm::puc_bytecode_loading()`. Default off — PUC
/// bytecode is a strictly larger trust surface than luna's own (the v1.3
/// audit calls this out as the embedder gate per §"Cross-dialect risks").
///
/// Whichever reader produced it, the prototype tree is verified before it is
/// returned (see the `verify` module): a chunk breaking an invariant the VM
/// relies on fails to load with `bad binary format (...)`.
pub fn undump(
    bytes: &[u8],
    heap: &mut Heap,
    version: LuaVersion,
    allow_puc: bool,
) -> Result<Gc<Proto>, String> {
    if bytes.first() != Some(&0x1b) {
        return Err("not a binary chunk".to_string());
    }
    let header = luna::header_for(version);
    let tag_at = header.len();
    let luna_body = bytes.len() >= tag_at + luna::BODY_TAG.len()
        && &bytes[tag_at..tag_at + luna::BODY_TAG.len()] == luna::BODY_TAG;
    // luna writes 0x55 for 5.1/5.2 as well (calls.lua does not pin those
    // dialects' header layouts), so any other version byte cannot be luna's.
    let written_version_byte = header[4];
    let puc_signature =
        bytes.len() >= 5 && &bytes[0..4] == b"\x1bLua" && matches!(bytes[4], 0x51..=0x55);
    let foreign_puc = puc_signature
        && !luna_body
        && (bytes[4] != written_version_byte || bytes.len() >= tag_at + luna::BODY_TAG.len());
    if foreign_puc && !allow_puc {
        return Err("PUC bytecode loading is disabled \
             (call vm.set_puc_bytecode_loading(true) to enable)"
            .to_string());
    }
    let proto = if foreign_puc {
        puc::undump_puc(bytes, heap)?
    } else {
        luna::undump(bytes, heap, version)?
    };
    verify::verify(&proto)?;
    Ok(proto)
}
