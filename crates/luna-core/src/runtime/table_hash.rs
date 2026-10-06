//! Main positions in the hash part. Numbers and booleans go where the
//! table's PUC version puts them: which keys collide decides when the hash
//! part fills up and the table is rehashed, and so how large its array
//! part is when `#t` searches it. Other keys keep luna's own hashing (PUC
//! hashes them by address, or by string hashes seeded per run from 5.2 on).

use super::*;

impl Table {
    pub(super) fn main_position(&self, k: Value) -> usize {
        let n = self.nodes().len();
        debug_assert!(n > 0);
        let mask = n - 1;
        // PUC `hashmod`: modulo the largest odd number below the size
        let odd = (mask | 1) as u64;
        match k {
            Value::Int(i) => match self.dialect() {
                Dialect::L51 => (hashnum_51(i as f64) % odd) as usize,
                Dialect::L52 => (hashnum_52(i as f64) % odd) as usize,
                Dialect::L53 => i as usize & mask,
                // `hashint`: the integer as unsigned, modulo
                Dialect::L54 | Dialect::L55 => (i as u64 % odd) as usize,
            },
            Value::Float(f) => match self.dialect() {
                Dialect::L51 => (hashnum_51(f) % odd) as usize,
                Dialect::L52 => (hashnum_52(f) % odd) as usize,
                _ => (u64::from(l_hashfloat(f)) % odd) as usize,
            },
            Value::Bool(b) => b as usize & mask,
            k => hash_key(k) as usize & mask,
        }
    }
}

/// PUC 5.1 `hashnum`: the two 32-bit halves of the double added (0 and -0
/// go to the first node).
fn hashnum_51(n: f64) -> u64 {
    if n == 0.0 {
        return 0;
    }
    let b = n.to_bits();
    u64::from((b as u32).wrapping_add((b >> 32) as u32))
}

/// PUC 5.2 `hashnum` with `luai_hashnum`'s IEEE 754 form (every platform
/// `make posix` / `make linux` builds for): the halves of `n + 1.0` added
/// as `int`s, then made non-negative.
fn hashnum_52(n: f64) -> u64 {
    let b = (n + 1.0).to_bits();
    let i = (b as u32 as i32).wrapping_add((b >> 32) as u32 as i32);
    // INT_MIN becomes 0, any other negative its absolute value
    u64::from(if i == i32::MIN { 0 } else { i.unsigned_abs() })
}

/// PUC 5.3+ `l_hashfloat`: the mantissa scaled to 31 bits plus the
/// exponent; `inf` and `nan` hash to 0.
fn l_hashfloat(n: f64) -> u32 {
    let (m, e) = frexp(n);
    let m = m * 2_147_483_648.0;
    // `lua_numbertointeger`: within [-2^63, 2^63)
    let lim = -(i64::MIN as f64);
    if !(-lim..lim).contains(&m) {
        return 0;
    }
    let u = (e as u32).wrapping_add(m as i64 as u32);
    if u <= i32::MAX as u32 { u } else { !u }
}

/// C `frexp`: `n = m * 2^e` with `0.5 <= |m| < 1`; zero, `inf` and `nan`
/// come back as they are with exponent 0.
fn frexp(n: f64) -> (f64, i32) {
    if n == 0.0 || !n.is_finite() {
        return (n, 0);
    }
    let bits = n.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        // subnormal: scale into the normal range first
        let (m, e) = frexp(n * f64::from_bits(0x4350_0000_0000_0000)); // 2^54
        return (m, e - 54);
    }
    let m = f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52));
    (m, exp - 1022)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frexp_matches_c() {
        assert_eq!(frexp(1.0), (0.5, 1));
        assert_eq!(frexp(-3.0), (-0.75, 2));
        assert_eq!(frexp(0.375), (0.75, -1));
        assert_eq!(frexp(f64::MIN_POSITIVE / 4.0), (0.5, -1023));
    }

    #[test]
    fn number_hashes_match_puc() {
        // values PUC computes for these keys: 5.1 adds the halves of the
        // double; 5.2 those of n + 1.0; 5.3+ frexp(n) scaled
        assert_eq!(hashnum_51(1.0), 0x3ff0_0000);
        assert_eq!(hashnum_52(1.0), 0x4000_0000);
        assert_eq!(l_hashfloat(1.5), 0x6000_0001);
        assert_eq!(l_hashfloat(f64::INFINITY), 0);
    }
}
