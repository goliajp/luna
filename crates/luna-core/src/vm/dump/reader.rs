//! Byte-stream reader shared by `super::luna` (luna's own body format) and
//! the per-dialect PUC translators landing in Phase LB Wave 2
//! (`super::puc::puc_5{1,2,3,4,5}`).
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

    /// Borrow the underlying byte slice. Used by `puc_51` to rewind a
    /// look-ahead peek (PUC 5.1's nested-proto recursion needs to look
    /// one byte ahead — the child's `nups` — before letting the
    /// recursive r_proto consume the full child header from scratch).
    #[allow(dead_code)]
    pub(super) fn peek_underlying_slice(&self) -> &'a [u8] {
        self.b
    }

    /// Advance the reader to byte position `to`. Mirrors the rewind /
    /// re-seek pattern needed by `puc_51`'s look-ahead.
    #[allow(dead_code)]
    pub(super) fn skip_to(&mut self, to: usize) -> Result<(), String> {
        if to < self.p || to > self.b.len() {
            return Err(format!(
                "skip_to {to} out of range (cur {}, len {})",
                self.p,
                self.b.len()
            ));
        }
        self.p = to;
        Ok(())
    }
}

/// PUC `lundump.c::loadVarint` / `loadSize` / `loadInt` MSB-first
/// big-endian varint used by 5.5 for string lengths, instruction counts,
/// line offsets, etc. The encoding (per `ldump.c::dumpVarint`):
///
/// > Bytes are written most-significant-first; each non-terminal byte has
/// > the high bit (0x80) **set**; the last byte has the high bit **clear**.
/// > Each byte carries 7 payload bits, and the accumulator is built as
/// > `acc = (acc << 7) | (b & 0x7f)`.
///
/// (Note: this is the MIRROR of LEB128 / DWARF "continuation = high bit
/// set, LSB-first". Wave 1's stub doc-comment had it backwards; corrected
/// here from a direct read of `lua-5.5.0/src/ldump.c::dumpVarint` and
/// `lua-5.5.0/src/lundump.c::loadVarint`.)
///
/// Hand-rolled to keep the luna-core 0-dep contract (no `leb128` crate).
/// Caps at 10 payload bytes (u64 saturation); rejects overflow.
#[allow(dead_code)] // Phase LB Wave 2 (5.4 / 5.5 translators) call this
pub(super) fn read_puc_varint(r: &mut Reader) -> Result<u64, String> {
    let mut acc: u64 = 0;
    for _ in 0..10 {
        let byte = r.u8()?;
        // Detect overflow: about to shift the accumulator left by 7 bits.
        // If any of the top 7 bits of `acc` are set, the new bits would
        // be lost — that's a u64 overflow.
        if acc >> 57 != 0 {
            return Err("puc varint value overflows u64".to_string());
        }
        acc = (acc << 7) | (byte & 0x7f) as u64;
        if byte & 0x80 == 0 {
            // high bit clear = last byte
            return Ok(acc);
        }
    }
    Err("puc varint value too long (max 10 bytes)".to_string())
}
