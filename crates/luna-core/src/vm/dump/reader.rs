//! Byte-stream reader shared by `super::luna` (luna's own body format) and
//! the per-dialect PUC translators landing in Phase LB Wave 2
//! (`super::puc_5{1,2,3,4,5}`).
//!
//! Stays stdlib-only — the luna-core 0-dep contract forbids pulling in
//! `byteorder`, `nom`, or a ULEB128 crate.

/// Cursor over a slice of bytes with truncation-safe primitive readers.
pub(super) struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    /// Start a reader at byte offset 0.
    #[allow(dead_code)] // not yet used; Phase LB Wave 2 will via puc::undump_puc
    pub(super) fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }

    /// Start a reader at byte offset `p` (luna's `undump` skips the
    /// header + body-tag bytes before constructing the reader).
    pub(super) fn at(b: &'a [u8], p: usize) -> Self {
        Self { b, p }
    }

    /// Current byte position.
    pub(super) fn pos(&self) -> usize {
        self.p
    }

    pub(super) fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.p.checked_add(n).ok_or("truncated chunk")?;
        let slice = self.b.get(self.p..end).ok_or("truncated chunk")?;
        self.p = end;
        Ok(slice)
    }

    pub(super) fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub(super) fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub(super) fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

/// PUC `lundump.c::loadSize` / `loadUnsigned` variant of ULEB128 used by
/// 5.4+ for string lengths, instruction counts, etc. The continuation
/// rule is INVERTED vs the standard ULEB128 found in DWARF / WASM:
///
/// > In PUC each byte's high bit (0x80) signals **terminator** — the last
/// > byte of the value. A byte with the high bit clear is a continuation
/// > byte. The low 7 bits are payload.
///
/// Hand-rolled to keep the luna-core 0-dep contract (no `leb128` crate).
/// Caps at 10 payload bytes (u64 saturation); rejects overflow.
#[allow(dead_code)] // Phase LB Wave 2 (5.4 / 5.5 translators) will call this
pub(super) fn read_uleb128(r: &mut Reader) -> Result<u64, String> {
    let mut acc: u64 = 0;
    for i in 0..10 {
        let byte = r.u8()?;
        let payload = (byte & 0x7f) as u64;
        // Detect overflow: shifting `payload` left by `7*i` and OR-ing into
        // `acc` must not lose bits. Last legal shift for u64 is 63.
        let shift = 7u32 * i as u32;
        if shift >= 64 {
            // would shift the entire payload off the top of u64
            if payload != 0 {
                return Err("uleb128 value overflows u64".to_string());
            }
        } else {
            let shifted = payload
                .checked_shl(shift)
                .ok_or_else(|| "uleb128 value overflows u64".to_string())?;
            // bits we'd clobber if shifted overflowed silently
            if shift > 0 && (payload >> (64 - shift)) != 0 {
                return Err("uleb128 value overflows u64".to_string());
            }
            acc |= shifted;
        }
        if byte & 0x80 != 0 {
            // high bit set = terminator in PUC variant
            return Ok(acc);
        }
    }
    Err("uleb128 value too long (max 10 bytes)".to_string())
}
