//! The `%a` conversion: a float in hexadecimal.

/// The 53-bit significand (leading bit at bit 52) and binary exponent of a
/// finite positive `x`; subnormals are normalised, as the reference libc
/// prints them (`0x1p-1074`, not `0x0.0000000000001p-1022`).
fn significand(x: f64) -> (u64, i64) {
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i64;
    let mant = bits & ((1 << 52) - 1);
    if exp == 0 {
        let shift = i64::from(mant.leading_zeros()) - 11;
        (mant << shift, -1022 - shift)
    } else {
        (mant | (1 << 52), exp - 1023)
    }
}

/// `%a`: prefix ("0x") and the rest. A precision rounds the significand to
/// that many hex digits, half to even; a carry out of the leading digit
/// leaves it at 2 (`0x2.0p+0`) as libc does.
pub(super) fn hex(
    x: f64,
    prec: Option<usize>,
    hash: bool,
    upper: bool,
) -> (&'static [u8], Vec<u8>) {
    let (m, e) = if x == 0.0 { (0, 0) } else { significand(x) };
    // fraction digits: as many as asked, or all significant ones
    let nfrac = prec.unwrap_or_else(|| {
        (0..=13)
            .find(|&n| m & ((1 << (52 - 4 * n)) - 1) == 0)
            .expect("n == 13 matches")
    });
    let kept = nfrac.min(13);
    let shift = 52 - 4 * kept as u32;
    let mut q = m >> shift;
    if shift > 0 {
        let rem = m & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        if rem > half || (rem == half && q & 1 == 1) {
            q += 1;
        }
    }
    let set: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut b = format!("{}", q >> (4 * kept)).into_bytes();
    if nfrac > 0 || hash {
        b.push(b'.');
    }
    for i in (0..kept).rev() {
        b.push(set[((q >> (4 * i)) & 0xF) as usize]);
    }
    b.extend(std::iter::repeat_n(b'0', nfrac - kept));
    b.push(if upper { b'P' } else { b'p' });
    b.extend_from_slice(format!("{}{}", if e < 0 { '-' } else { '+' }, e.abs()).as_bytes());
    (if upper { b"0X" } else { b"0x" }, b)
}
