//! AArch64 logical-immediate encoding.

/// N:immr:imms of a logical immediate, if `imm` has one at `width` bits.
pub(super) fn logical_imm(imm: u64, width: u32) -> Option<u32> {
    let imm = if width == 32 { imm & 0xffff_ffff } else { imm };
    let all = if width == 32 { 0xffff_ffff } else { u64::MAX };
    if imm == 0 || imm == all {
        return None;
    }
    let mut size = width;
    loop {
        size /= 2;
        let mask = (1u64 << size) - 1;
        if (imm & mask) != ((imm >> size) & mask) {
            size *= 2;
            break;
        }
        if size <= 2 {
            break;
        }
    }
    let mask = u64::MAX >> (64 - size);
    let mut v = imm & mask;
    let shifted_mask = |x: u64| x != 0 && ((x | (x - 1)).wrapping_add(1) & (x | (x - 1))) == 0;
    let (i, cto);
    if shifted_mask(v) {
        i = v.trailing_zeros();
        cto = (v >> i).trailing_ones();
    } else {
        v |= !mask;
        if !shifted_mask(!v) {
            return None;
        }
        let clo = v.leading_ones();
        i = 64 - clo;
        cto = clo + v.trailing_ones() - (64 - size);
    }
    let immr = (size - i) & (size - 1);
    let nimms = (!(u64::from(size) - 1) << 1) | u64::from(cto - 1);
    let n = ((nimms >> 6) & 1) ^ 1;
    Some(((n as u32) << 12) | (immr << 6) | (nimms as u32 & 0x3f))
}

#[cfg(test)]
mod tests {
    use super::logical_imm;

    #[test]
    fn logical_immediates_encode_like_the_architecture_manual() {
        // values checked against an assembler's output
        assert_eq!(logical_imm(0xff, 64), Some(0x1007));
        assert_eq!(logical_imm(0xff, 32), Some(0x007));
        assert_eq!(logical_imm(0x5555_5555_5555_5555, 64), Some(0x03c));
        assert_eq!(logical_imm(0xffff_ffff_0000_0000, 64), Some(0x181f));
        assert_eq!(logical_imm(0, 64), None);
        assert_eq!(logical_imm(u64::MAX, 64), None);
        assert_eq!(logical_imm(0x1234, 64), None);
    }
}
