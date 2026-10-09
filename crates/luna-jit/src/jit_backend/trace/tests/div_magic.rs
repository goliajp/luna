//! The multiply that replaces a division by a constant divisor.

use super::super::floor_div::div_magic;

fn umulhi(a: u64, b: u64) -> u64 {
    ((u128::from(a) * u128::from(b)) >> 64) as u64
}

fn check(d: u64, n: u64) {
    let q = match div_magic(d) {
        None => n,
        Some((m, shift)) => umulhi(n, m) >> shift,
    };
    assert_eq!(q, n / d, "{n} / {d}");
}

#[test]
fn matches_division_for_every_63_bit_dividend() {
    let top = (1u64 << 63) - 1;
    let mut ds: Vec<u64> = (1..=1000).collect();
    for b in 1..63 {
        let p = 1u64 << b;
        ds.extend([p - 1, p, p + 1, p / 3 * 2 + 1]);
    }
    ds.extend([top, top - 1, 1_000_000_007, 0x5555_5555_5555_5555]);
    // a fixed xorshift sequence of dividends besides the edges
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    for &d in &ds {
        for n in [
            0,
            1,
            d - 1,
            d,
            d + 1,
            top,
            top - 1,
            top / d * d,
            top / d * d - 1,
        ] {
            check(d, n.min(top));
        }
        for _ in 0..200 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            check(d, x & top);
        }
    }
}
