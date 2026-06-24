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

use crate::runtime::function::Proto;
use crate::runtime::heap::{Gc, Heap};
use crate::version::LuaVersion;

/// Serialise a function prototype to a binary chunk.
///
/// Delegates to [`luna::dump`]; output is luna's own body format (PUC
/// dialect header + `"\x00LunaV1\x00"` sentinel + luna body). Not
/// PUC-loadable — see RFC v1.3 §"open questions"-4 for the PUC-output
/// `string.dump` v1.4 candidate.
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
/// Routes by the leading 5 bytes:
/// - `\x1bLua` + the running dialect's version byte → [`luna::undump`]
///   (this is what luna's own `dump` emits, and the only path that
///   accepts the `BODY_TAG` sentinel)
/// - `\x1bLua` + a `0x51..0x55` version byte that does NOT match the
///   running dialect → [`puc::undump_puc`] (gated by `allow_puc`;
///   rejected with a clear error when disabled)
/// - anything else → `Err("not a binary chunk")`
///
/// **Routing is decided by the version byte alone** — we do not try luna
/// first and fall back to PUC on error, because luna's "truncated chunk"
/// / "bad header" errors must surface verbatim for the
/// `calls.lua` corrupted-header + truncated-chunk round-trip tests.
///
/// `allow_puc` mirrors `Vm::puc_bytecode_loading()`. Default off — PUC
/// bytecode is a strictly larger trust surface than luna's own (the v1.3
/// audit calls this out as the embedder gate per §"Cross-dialect risks").
pub fn undump(
    bytes: &[u8],
    heap: &mut Heap,
    version: LuaVersion,
    allow_puc: bool,
) -> Result<Gc<Proto>, String> {
    if bytes.first() != Some(&0x1b) {
        return Err("not a binary chunk".to_string());
    }
    // Version-byte dispatch. luna's own dumper writes the running
    // dialect's byte (e.g. Lua54 → 0x54); any other 0x51..0x55 byte
    // signals a foreign PUC chunk. Short / mangled chunks (where
    // `bytes[4]` is absent or junk) fall to `luna::undump`, which produces
    // the truncated / bad-header errors the test suite asserts on.
    // Per `luna::header_for`, luna dumps 5.1 / 5.2 chunks with the
    // `0x55` version byte (calls.lua doesn't pin those two dialects'
    // header layouts, so luna piggy-backs on HEADER_55). The version
    // byte luna would WRITE for the running dialect:
    let written_version_byte = match version {
        LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua55 => 0x55,
        LuaVersion::Lua53 => 0x53,
        LuaVersion::Lua54 => 0x54,
    };
    let foreign_puc = bytes.len() >= 5
        && &bytes[0..4] == b"\x1bLua"
        && matches!(bytes[4], 0x51..=0x55)
        && bytes[4] != written_version_byte;
    if foreign_puc {
        if !allow_puc {
            return Err("PUC bytecode loading is disabled \
                 (call vm.set_puc_bytecode_loading(true) to enable)"
                .to_string());
        }
        return puc::undump_puc(bytes, heap);
    }
    luna::undump(bytes, heap, version)
}
