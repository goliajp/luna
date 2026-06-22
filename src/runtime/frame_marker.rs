//! P17-D v2 Phase 1 — type foundation for LJ-style unified stack-frame
//! memory.
//!
//! In LuaJIT 2.1's LJ_FR2 model, each Lua activation record stores TWO
//! pieces of metadata INLINE in the value stack, at fixed offsets
//! relative to the frame's `base`:
//!
//! - `stack[base-2]` — the function being called (a closure GCRef).
//!   In our terms: `Value::Closure(Gc<LuaClosure>)` for Lua frames.
//! - `stack[base-1]` — a 64-bit "frame marker" packing:
//!   - bits 0-2:  the frame kind (LJ FRAME_LUA / FRAME_C / FRAME_CONT
//!     / FRAME_VARG). Encoded as the type tag's lower 3 bits.
//!   - bits 3-63: the frame's PC (for Lua frames, a u32 bytecode index
//!     fits comfortably in 61 bits) or a frame-delta (for native /
//!     vararg / continuation frames).
//!
//! See `lj_frame.h:33-110` for the upstream macros; this module mirrors
//! their semantics in luna terms. See
//! `docs/rfcs/20260622-p17-d-v2-lj-unified-stack/design.md` §1 for the
//! migration plan that consumes these primitives.
//!
//! **Phase 1 (this module)** — pure type definitions + bit-packing
//! helpers + roundtrip tests. NOT yet consumed by `Vm`. Adding this
//! module has no behavior or perf impact; Phase 2-4 wires it into
//! the frame setup/teardown paths.

/// The kind of an activation record, encoded into the lower 3 bits of
/// a [`FrameMarker`] u64. Mirrors LuaJIT's `FRAME_*` enum from
/// `lj_frame.h:18-22`.
///
/// `Lua` corresponds to a Lua-level function activation; its PC field
/// holds the next bytecode index to execute on resume.
/// `Cont` corresponds to a native continuation frame (luna's `Cont`
/// variant of [`CallFrame`]). luna doesn't currently emit separate
/// FRAME_C / FRAME_VARG markers — those PUC distinctions are encoded
/// elsewhere (e.g., `Frame.from_c`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum FrameKind {
    /// A Lua function frame. The upper bits of the marker hold the
    /// frame's PC (u32 bytecode index into the proto's `code`).
    Lua = 0,
    /// A native continuation frame (pcall, xpcall, metamethod
    /// continuation, etc.). The upper bits hold a frame delta to the
    /// previous frame's base — not a PC. Continuations have no Lua PC.
    Cont = 1,
}

impl FrameKind {
    /// Decode a tag value (the lower 3 bits of a marker) back into a
    /// `FrameKind`. Returns `None` for invalid tag values; the caller
    /// is expected to treat that as a corrupt frame.
    #[inline]
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(FrameKind::Lua),
            1 => Some(FrameKind::Cont),
            _ => None,
        }
    }

    /// The 3-bit tag value used in marker encoding.
    #[inline]
    pub fn tag(self) -> u8 {
        self as u8
    }
}

/// Number of low bits reserved for the frame kind tag.
const FRAME_KIND_BITS: u32 = 3;
const FRAME_KIND_MASK: u64 = (1 << FRAME_KIND_BITS) - 1;

/// A 64-bit packed activation record marker that lives in the value
/// stack slot `base-1` of any Lua frame (LJ_FR2 model). Carries the
/// frame kind (lower 3 bits) + PC-or-delta payload (upper 61 bits).
///
/// Construction goes through [`FrameMarker::new_lua`] /
/// [`FrameMarker::new_cont`] which encode their args correctly;
/// destruction goes through [`FrameMarker::kind`] / [`FrameMarker::payload`].
///
/// Internally a u64 so it round-trips through `Value::Int(i64)` safely
/// without losing bits — luna's `Value::Int` is a full i64 (no NaN
/// boxing), so the bit pattern survives the stack store/load cleanly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameMarker(u64);

impl FrameMarker {
    /// Build a `FrameKind::Lua` marker with the given PC. PC must fit
    /// in 61 bits (a u32 always does). Panics in debug mode on
    /// overflow; production code is expected to pass a u32.
    #[inline]
    pub fn new_lua(pc: u32) -> Self {
        let payload = (pc as u64) << FRAME_KIND_BITS;
        FrameMarker(payload | FrameKind::Lua.tag() as u64)
    }

    /// Build a `FrameKind::Cont` marker with the given frame delta
    /// (slots between this frame's base and the previous frame's base).
    /// Delta must fit in 61 bits.
    #[inline]
    pub fn new_cont(delta: u32) -> Self {
        let payload = (delta as u64) << FRAME_KIND_BITS;
        FrameMarker(payload | FrameKind::Cont.tag() as u64)
    }

    /// Read the frame kind (lower 3 bits).
    #[inline]
    pub fn kind(self) -> Option<FrameKind> {
        FrameKind::from_tag((self.0 & FRAME_KIND_MASK) as u8)
    }

    /// Read the PC/delta payload (upper 61 bits, shifted down).
    /// For Lua frames this is the bytecode PC; for Cont frames the
    /// caller-base delta.
    #[inline]
    pub fn payload(self) -> u32 {
        (self.0 >> FRAME_KIND_BITS) as u32
    }

    /// Raw bits, suitable for storing in a `Value::Int(i64)` slot.
    /// The reverse direction is [`FrameMarker::from_raw`].
    #[inline]
    pub fn to_raw(self) -> i64 {
        self.0 as i64
    }

    /// Reconstruct a `FrameMarker` from raw bits read out of a value
    /// stack slot. The caller is responsible for ensuring the slot
    /// actually held a marker (otherwise the kind decode may fail).
    #[inline]
    pub fn from_raw(raw: i64) -> Self {
        FrameMarker(raw as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_marker_roundtrip_basic() {
        let m = FrameMarker::new_lua(42);
        assert_eq!(m.kind(), Some(FrameKind::Lua));
        assert_eq!(m.payload(), 42);
    }

    #[test]
    fn cont_marker_roundtrip_basic() {
        let m = FrameMarker::new_cont(7);
        assert_eq!(m.kind(), Some(FrameKind::Cont));
        assert_eq!(m.payload(), 7);
    }

    #[test]
    fn raw_bits_roundtrip_through_i64() {
        for pc in [0u32, 1, 100, u16::MAX as u32, u32::MAX] {
            let m = FrameMarker::new_lua(pc);
            let raw = m.to_raw();
            let m2 = FrameMarker::from_raw(raw);
            assert_eq!(m2.kind(), Some(FrameKind::Lua), "kind survives pc={}", pc);
            assert_eq!(m2.payload(), pc, "payload survives pc={}", pc);
        }
    }

    #[test]
    fn cont_payload_survives_full_u32() {
        for delta in [0u32, 1, 100, u32::MAX] {
            let m = FrameMarker::new_cont(delta);
            let m2 = FrameMarker::from_raw(m.to_raw());
            assert_eq!(m2.kind(), Some(FrameKind::Cont));
            assert_eq!(m2.payload(), delta);
        }
    }

    #[test]
    fn kind_tag_values_match_lj() {
        // LJ frame.h: FRAME_LUA=0, FRAME_C=1, FRAME_CONT=2, FRAME_VARG=3.
        // luna currently uses 0=Lua, 1=Cont (Cont covers both PUC's C-
        // continuation and metamethod-continuation cases; we don't
        // emit FRAME_C / FRAME_VARG markers yet).
        assert_eq!(FrameKind::Lua.tag(), 0);
        assert_eq!(FrameKind::Cont.tag(), 1);
        assert_eq!(FrameKind::from_tag(0), Some(FrameKind::Lua));
        assert_eq!(FrameKind::from_tag(1), Some(FrameKind::Cont));
        assert_eq!(FrameKind::from_tag(2), None, "FRAME_CONT_LJ not emitted yet");
        assert_eq!(FrameKind::from_tag(7), None, "invalid tag rejected");
    }

    #[test]
    fn kind_decode_safe_on_invalid() {
        // Garbage marker (e.g., when stack[base-1] was overwritten by
        // a regular Value::Int with low bits != 0 or 1) returns None
        // rather than panicking. Callers that need correctness must
        // validate before consuming.
        let garbage = FrameMarker::from_raw(0x_8000_0000_0000_0007u64 as i64);
        assert_eq!(garbage.kind(), None);
    }
}
