//! The length border (`#t`), as each PUC version's `luaH_getn` picks it
//! when the table has several: the border depends on the array part's size
//! and, in 5.4 and 5.5, on state the previous lookups left behind.

use super::*;

impl Table {
    /// A border: `n` where `t[n]` is non-nil and `t[n+1]` is nil (PUC `luaH_getn`).
    /// This is Lua `#` semantics, not a container size — an `is_empty`
    /// counterpart would be meaningless. Of several borders it returns the
    /// one the table's PUC version returns, given the same history.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> i64 {
        match self.dialect() {
            Dialect::L54 => {
                // a leading run and nothing after it: its end is the only
                // border, which 5.4's search returns, moving `alimit` as
                // `alimit_after_dense_len` says
                let p = self.aprefix;
                if self.acount == p && (p as usize) < self.asize() {
                    let l = alimit_after_dense_len(self.alimit.get(), p, self.asize as u32);
                    self.alimit.set(l);
                    return p as i64;
                }
                self.len_54()
            }
            Dialect::L55 => {
                // a leading run and nothing after it: its end is the only
                // border, the one 5.5's search finds and keeps as the hint
                if self.acount == self.aprefix && (self.aprefix as usize) < self.asize() {
                    self.lenhint.set(self.aprefix);
                    return self.aprefix as i64;
                }
                self.len_55()
            }
            d => {
                if self.acount == self.aprefix && (self.aprefix as usize) < self.asize() {
                    return self.aprefix as i64;
                }
                self.len_51(d)
            }
        }
    }

    /// Whether array slot `k` (1-based) is nil.
    #[inline]
    fn slot_nil(&self, k: usize) -> bool {
        self.atags()[k - 1] == raw::NIL
    }

    /// PUC `binsearch`: a border in `[i, j)` of the array part, given
    /// `t[i]` non-nil (or `i == 0`) and `t[j]` nil.
    fn binsearch(&self, mut i: usize, mut j: usize) -> usize {
        while j - i > 1 {
            let m = (i + j) / 2;
            if self.slot_nil(m) {
                j = m;
            } else {
                i = m;
            }
        }
        i
    }

    /// 5.1–5.3: binary search in the array part when its last slot is nil,
    /// else an unbound search through the hash part.
    pub(super) fn len_51(&self, d: Dialect) -> i64 {
        let asize = self.asize();
        if asize > 0 && self.slot_nil(asize) {
            return self.binsearch(0, asize) as i64;
        }
        if self.nodes().is_empty() {
            return asize as i64;
        }
        // `unbound_search`: 5.1/5.2 count in an `unsigned int` and give up
        // doubling past INT_MAX, 5.3 past LUA_MAXINTEGER / 2
        let limit = if d <= Dialect::L52 {
            i32::MAX as u64
        } else {
            i64::MAX as u64 / 2
        };
        let mut i = asize as u64;
        let mut j = i + 1;
        while !self.get_int(j as i64).is_nil() {
            i = j;
            if d <= Dialect::L52 {
                j *= 2;
                if j > limit {
                    return self.linear_border();
                }
            } else {
                if j > limit {
                    return self.linear_border();
                }
                j *= 2;
            }
        }
        self.hash_binsearch(i, j)
    }

    /// The fallback of a doubling search that overflowed: the first border
    /// counting up from 1.
    fn linear_border(&self) -> i64 {
        let mut i = 1i64;
        while !self.get_int(i).is_nil() {
            i += 1;
        }
        i - 1
    }

    /// Binary search between a present index `i` and an absent `j`.
    fn hash_binsearch(&self, mut i: u64, mut j: u64) -> i64 {
        while j - i > 1 {
            let m = i + (j - i) / 2;
            if self.get_int(m as i64).is_nil() {
                j = m;
            } else {
                i = m;
            }
        }
        i as i64
    }

    /// 5.4 and 5.5 `hash_search` (5.5 with a zero seed): doubling from a
    /// present `j` (0 counts as present) until an absent index, then a
    /// binary search.
    fn hash_search(&self, mut j: u64) -> i64 {
        const MAX: u64 = i64::MAX as u64;
        if j == 0 {
            j = 1;
        }
        let mut i;
        loop {
            i = j;
            if j <= MAX / 2 {
                j *= 2;
            } else {
                j = MAX;
                if self.get_int(j as i64).is_nil() {
                    break;
                }
                return j as i64;
            }
            if self.get_int(j as i64).is_nil() {
                break;
            }
        }
        self.hash_binsearch(i, j)
    }

    /// 5.4: start from `alimit`, which this search may lower to the border
    /// it finds (see `alimit`); the array part's real size is `asize`.
    pub(super) fn len_54(&self) -> i64 {
        let real = self.asize();
        let mut limit = self.alimit.get() as usize;
        let pow2 = real.is_power_of_two();
        if limit > 0 && self.slot_nil(limit) {
            if limit >= 2 && !self.slot_nil(limit - 1) {
                if pow2 && !(limit - 1).is_power_of_two() {
                    self.alimit.set(limit as u32 - 1);
                }
                return limit as i64 - 1;
            }
            let border = self.binsearch(0, limit);
            if pow2 && border > real / 2 {
                self.alimit.set(border as u32);
            }
            return border as i64;
        }
        if limit != real {
            if self.slot_nil(limit + 1) {
                return limit as i64;
            }
            limit = real;
            if self.slot_nil(limit) {
                let border = self.binsearch(self.alimit.get() as usize, limit);
                self.alimit.set(border as u32);
                return border as i64;
            }
            // 5.4.9 leaves `alimit` where it was here
        }
        if self.nodes().is_empty() || self.get_int(limit as i64 + 1).is_nil() {
            return limit as i64;
        }
        self.hash_search(limit as u64)
    }

    /// 5.5: look a few slots either side of `lenhint` first, then search;
    /// the border found becomes the next hint.
    pub(super) fn len_55(&self) -> i64 {
        const VICINITY: usize = 4;
        let asize = self.asize();
        if asize > 0 {
            let mut limit = (self.lenhint.get() as usize).max(1);
            if self.slot_nil(limit) {
                for _ in 0..VICINITY {
                    if limit <= 1 {
                        break;
                    }
                    limit -= 1;
                    if !self.slot_nil(limit) {
                        return self.new_hint(limit);
                    }
                }
                return self.new_hint(self.binsearch(0, limit));
            }
            for _ in 0..VICINITY {
                if limit >= asize {
                    break;
                }
                limit += 1;
                if self.slot_nil(limit) {
                    return self.new_hint(limit - 1);
                }
            }
            if self.slot_nil(asize) {
                return self.new_hint(self.binsearch(limit, asize));
            }
            self.lenhint.set(asize as u32);
        }
        if self.nodes().is_empty() || self.get_int(asize as i64 + 1).is_nil() {
            return asize as i64;
        }
        self.hash_search_55(asize as u64)
    }

    fn new_hint(&self, border: usize) -> i64 {
        self.lenhint.set(border as u32);
        border as i64
    }

    /// 5.5 `hash_search`: `t[asize + 1]` is present; probe a random step
    /// past it, then double, adding a random bit each time, until an
    /// absent index, then binary search. The randomness keeps a crafted
    /// table (keys 1, 2, 4, ..., 2^62) from making `#t` its huge border;
    /// PUC draws it from the state's seed, luna from the table's address.
    fn hash_search_55(&self, asize: u64) -> i64 {
        const MAX: u64 = i64::MAX as u64;
        let addr = self as *const Table as usize as u64;
        let mut rnd = ((addr >> 4) ^ (addr >> 36)) as u32 | 1;
        let n = if asize > 0 {
            ceil_log2(asize) as u32
        } else {
            0
        };
        let mask = ((1u64 << n) - 1) as u32;
        let incr = u64::from(rnd & mask) + 1;
        let mut i = asize + 1;
        let mut j = if incr <= MAX - i { i + incr } else { i + 1 };
        rnd = rnd.checked_shr(n).unwrap_or(0);
        while !self.get_int(j as i64).is_nil() {
            i = j;
            if j < MAX / 2 {
                j = j * 2 + u64::from(rnd & 1);
                rnd >>= 1;
            } else {
                j = MAX;
                if self.get_int(j as i64).is_nil() {
                    break;
                }
                return j as i64;
            }
        }
        self.hash_binsearch(i, j)
    }
}

/// Where 5.4's `#t` (`len_54`) leaves `alimit` `l` when the array part
/// of size `a` holds exactly a leading run of `p < a` values: at `p` when
/// `l < p` (the search goes on to the end of the array part and finds
/// `p`); when `l > p`, at `p` if `a` is a power of two and either
/// `l - 1 == p` with `p` not a power of two, or `p` past half of `a`;
/// otherwise where it was. The trace JIT's inline `#t` computes the same.
pub(super) fn alimit_after_dense_len(l: u32, p: u32, a: u32) -> u32 {
    // PUC `ispow2`, which counts 0 as one
    let pow2 = |x: u32| x & x.wrapping_sub(1) == 0;
    let moved = if l < p {
        true
    } else if l > p {
        pow2(a) && if l - 1 == p { !pow2(p) } else { p > a / 2 }
    } else {
        false
    };
    if moved { p } else { l }
}
