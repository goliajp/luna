//! Binary-chunk dump / undump entry point.
//!
//! Sub-modules:
//! - `luna` — luna's own dump / undump (per-dialect PUC header + a
//!   luna-specific body sentinel-tagged `"\x00LunaV1\x00"`).
//! - `reader` — shared byte-stream reader + PUC `loadSize` ULEB128 port
//!   (0-dep — luna-core contract forbids `leb128` / `byteorder` crates).
//! - `puc` — magic-byte → per-dialect PUC undumper dispatch.
//! - `puc_writer` — `Proto` → PUC chunk of the running dialect
//!   (`string.dump`).
//!
//! Public surface (used by `builtins.rs`, `exec.rs`, `lib_os_io.rs`,
//! `lib_string.rs`):
//! - [`dump`] — `Proto → Vec<u8>` (luna body format)
//! - `dump_puc` — `Proto → Vec<u8>` (the running dialect's PUC format)
//! - [`undump`] — bytes → `Gc<Proto>`, routes by leading magic byte
//! - [`is_binary_chunk`] — true for any `\x1b`-prefixed input (matches
//!   both luna and PUC bodies; loader uses this to decide
//!   "undump vs parse")

mod error;
mod header;
mod luna;
mod puc;
mod puc_writer;
mod reader;
mod verify;

use crate::runtime::function::Proto;
use crate::runtime::heap::{Gc, Heap};
use crate::version::LuaVersion;

/// Serialise a function prototype to a binary chunk.
///
/// Delegates to `luna::dump` (private sibling module); output is luna's
/// own body format (PUC dialect header + `"\x00LunaV1\x00"` sentinel +
/// luna body). Not PUC-loadable.
pub fn dump(proto: &Proto, strip: bool, version: LuaVersion) -> Vec<u8> {
    luna::dump(proto, strip, version)
}

/// Serialise a function prototype as the running dialect's PUC bytecode,
/// loadable by the stock interpreter of that version (`string.dump`).
/// `Err` when the function has no faithful encoding in that dialect's
/// instruction set, or the dialect (MacroLua) has no PUC format.
pub(crate) fn dump_puc(proto: &Proto, strip: bool, version: LuaVersion) -> Result<Vec<u8>, String> {
    puc_writer::dump(proto, strip, version)
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
///   header reaches luna's own header errors (calls.lua pins them).
/// - shorter than header + tag and not foreign → `luna::undump`, which
///   reports the truncation.
/// - otherwise a `\x1bLua` chunk of the running dialect's own PUC version
///   is what `string.dump` writes → `puc::undump_puc`, under the same gate
///   as luna's own chunks (the caller's bytecode-loading switch).
/// - a `\x1bLua` chunk of another PUC version → `puc::undump_puc`, gated
///   by `allow_puc`.
/// - anything else → `luna::undump` for its error.
///
/// `allow_puc` mirrors `Vm::puc_bytecode_loading()`. Default off — a chunk
/// from another dialect's toolchain is a larger trust surface than one the
/// running dialect's `string.dump` could have written.
///
/// Whichever reader produced it, the prototype tree is verified before it is
/// returned (see the `verify` module). A refused chunk's message is worded
/// as the running dialect's `lundump.c` words it, without the chunk-name
/// prefix `load` adds (see `undump_named`).
pub fn undump(
    bytes: &[u8],
    heap: &mut Heap,
    version: LuaVersion,
    allow_puc: bool,
) -> Result<Gc<Proto>, String> {
    undump_checked(bytes, heap, version, allow_puc).map_err(|r| match r {
        Refusal::Gate(msg) => msg,
        Refusal::Bad(bad) => bad.render(version),
    })
}

/// [`undump`] as `load` reports it: a malformed chunk's message starts with
/// `lundump.c`'s chunk name (`chunkname` without a leading `@` or `=`, or
/// `binary string` for a name starting with the escape byte).
pub(crate) fn undump_named(
    bytes: &[u8],
    heap: &mut Heap,
    version: LuaVersion,
    allow_puc: bool,
    chunkname: &[u8],
) -> Result<Gc<Proto>, Vec<u8>> {
    undump_checked(bytes, heap, version, allow_puc).map_err(|r| match r {
        Refusal::Gate(msg) => msg.into_bytes(),
        Refusal::Bad(bad) => {
            let mut out = error::lundump_name(chunkname).to_vec();
            out.extend_from_slice(b": ");
            out.extend_from_slice(bad.render(version).as_bytes());
            out
        }
    })
}

/// Whether `bytes` is the start of a binary chunk that ends too early: a
/// reader that delivered them would be asked for more.
pub(crate) fn truncated(bytes: &[u8], heap: &mut Heap, version: LuaVersion, allow_puc: bool) -> bool {
    matches!(
        undump_checked(bytes, heap, version, allow_puc),
        Err(Refusal::Bad(error::Bad::Truncated))
    )
}

/// A chunk refused by an embedder gate, or found malformed.
enum Refusal {
    Gate(String),
    Bad(error::Bad),
}

impl From<error::Bad> for Refusal {
    fn from(bad: error::Bad) -> Refusal {
        Refusal::Bad(bad)
    }
}

fn undump_checked(
    bytes: &[u8],
    heap: &mut Heap,
    version: LuaVersion,
    allow_puc: bool,
) -> Result<Gc<Proto>, Refusal> {
    if bytes.first() != Some(&0x1b) {
        return Err(Refusal::Gate("not a binary chunk".to_string()));
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
    let own_puc = puc_signature && !luna_body && Some(bytes[4]) == own_puc_version(version);
    let foreign_puc = puc_signature
        && !luna_body
        && !own_puc
        && (bytes[4] != written_version_byte || bytes.len() >= tag_at + luna::BODY_TAG.len());
    if foreign_puc && !allow_puc {
        return Err(Refusal::Gate(
            "PUC bytecode loading is disabled \
             (call vm.set_puc_bytecode_loading(true) to enable)"
                .to_string(),
        ));
    }
    let proto = if own_puc || foreign_puc {
        puc::undump_puc(bytes, heap)?
    } else {
        luna::undump(bytes, heap, version)?
    };
    verify::verify(&proto).map_err(error::Bad::Code)?;
    Ok(proto)
}

/// The version byte of the PUC format `string.dump` writes for `version`.
fn own_puc_version(version: LuaVersion) -> Option<u8> {
    match version {
        LuaVersion::Lua51 => Some(0x51),
        LuaVersion::Lua52 => Some(0x52),
        LuaVersion::Lua53 => Some(0x53),
        LuaVersion::Lua54 => Some(0x54),
        LuaVersion::Lua55 => Some(0x55),
        LuaVersion::MacroLua => None,
    }
}
