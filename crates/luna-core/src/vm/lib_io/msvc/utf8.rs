//! UTF-8 to UTF-16 as `MultiByteToWideChar(CP_UTF8, 0, ...)` converts it,
//! which the low-level read of a UTF-8 stream calls: a sequence that is
//! not UTF-8 becomes U+FFFD, one for each lead byte with the continuation
//! bytes it took, one for each other byte. Measured on Windows Server 2025:
//! a lead byte takes continuation bytes up to its length, stopping at a
//! byte that is not one; a second byte that is a continuation byte but
//! outside the lead's range (`E0`: `A0`–`BF`, `ED`: `80`–`9F`, `F0`:
//! `90`–`BF`, `F4`: `80`–`8F`) is taken into the replacement and ends it.
//! `C0`, `C1` and `F5` to `FF` lead nothing.

const REPLACEMENT: u16 = 0xFFFD;

/// How many bytes a lead byte heads, and the range of its second byte.
fn lead(b: u8) -> Option<(usize, u8, u8)> {
    Some(match b {
        0xC2..=0xDF => (2, 0x80, 0xBF),
        0xE0 => (3, 0xA0, 0xBF),
        0xE1..=0xEC | 0xEE..=0xEF => (3, 0x80, 0xBF),
        0xED => (3, 0x80, 0x9F),
        0xF0 => (4, 0x90, 0xBF),
        0xF1..=0xF3 => (4, 0x80, 0xBF),
        0xF4 => (4, 0x80, 0x8F),
        _ => return None,
    })
}

fn is_continuation(b: u8) -> bool {
    (0x80..=0xBF).contains(&b)
}

/// The UTF-16 code units for `bytes`.
pub(crate) fn to_utf16(bytes: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            out.push(u16::from(b));
            i += 1;
            continue;
        }
        let Some((len, lo, hi)) = lead(b) else {
            out.push(REPLACEMENT);
            i += 1;
            continue;
        };
        let second = bytes.get(i + 1).copied();
        if !second.is_some_and(is_continuation) {
            out.push(REPLACEMENT);
            i += 1;
            continue;
        }
        if !(lo..=hi).contains(&second.expect("a continuation byte")) {
            out.push(REPLACEMENT);
            i += 2;
            continue;
        }
        let mut taken = 2;
        while taken < len && bytes.get(i + taken).copied().is_some_and(is_continuation) {
            taken += 1;
        }
        if taken < len {
            out.push(REPLACEMENT);
            i += taken;
            continue;
        }
        let mut c = u32::from(b) & (0x7F >> len);
        for &t in &bytes[i + 1..i + len] {
            c = (c << 6) | u32::from(t & 0x3F);
        }
        let ch = char::from_u32(c)
            .expect("a lead and its continuation bytes in range spell a scalar value");
        let mut buf = [0u16; 2];
        out.extend_from_slice(ch.encode_utf16(&mut buf));
        i += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::to_utf16;

    /// What `MultiByteToWideChar(CP_UTF8, 0, ...)` gave for each sequence.
    #[test]
    fn invalid_sequences_convert_as_windows_converts_them() {
        const F: u16 = 0xFFFD;
        let cases: &[(&[u8], &[u16])] = &[
            (b"ab", &[0x61, 0x62]),
            (&[0xC3, 0xA9], &[0xE9]),
            (&[0xE2, 0x82, 0xAC], &[0x20AC]),
            (&[0xF0, 0x9F, 0x98, 0x80], &[0xD83D, 0xDE00]),
            (&[0x80], &[F]),
            (&[0x80, 0x80], &[F, F]),
            (&[0x61, 0x80, 0x62], &[0x61, F, 0x62]),
            (&[0xC3], &[F]),
            (&[0xC3, 0x61], &[F, 0x61]),
            (&[0xE2, 0x82], &[F]),
            (&[0xE2, 0x82, 0x61], &[F, 0x61]),
            (&[0xE2], &[F]),
            (&[0xF0, 0x9F, 0x98], &[F]),
            (&[0xF0, 0x9F, 0x98, 0x61], &[F, 0x61]),
            (&[0xF0, 0x9F], &[F]),
            (&[0xC0, 0x80], &[F, F]),
            (&[0xC1, 0xBF], &[F, F]),
            (&[0xE0, 0x80, 0x80], &[F, F]),
            (&[0xF0, 0x80, 0x80, 0x80], &[F, F, F]),
            (&[0xED, 0xA0, 0x80], &[F, F]),
            (&[0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80], &[F, F, F, F]),
            (&[0xF4, 0x90, 0x80, 0x80], &[F, F, F]),
            (&[0xF5, 0x80, 0x80, 0x80], &[F, F, F, F]),
            (&[0xF8, 0x88, 0x80, 0x80, 0x80], &[F, F, F, F, F]),
            (&[0xFE], &[F]),
            (&[0xFF], &[F]),
            (&[0xFF, 0x61], &[F, 0x61]),
            (&[0xC3, 0xC3, 0xA9], &[F, 0xE9]),
            (&[0xE2, 0x82, 0xC3, 0xA9], &[F, 0xE9]),
            (&[0xC3, 0xA9, 0x80], &[0xE9, F]),
            (&[0xEF, 0xBB, 0xBF, 0x61], &[0xFEFF, 0x61]),
            (&[0x61, 0, 0x62], &[0x61, 0, 0x62]),
        ];
        for (bytes, want) in cases {
            assert_eq!(to_utf16(bytes), *want, "{bytes:02X?}");
        }
    }
}
