//! Main positions in the hash part, and placing a new key there. Numbers and booleans go where the
//! table's PUC version puts them: which keys collide decides when the hash
//! part fills up and the table is rehashed, and so how large its array
//! part is when `#t` searches it. Other keys keep luna's own hashing (PUC
//! hashes them by address, or by string hashes seeded per run from 5.2 on).

use super::*;
use node::KHASH_MASK;

impl Table {
    /// [`Self::main_position`] with a string's taken in line: the common
    /// key, and the one insertion asks about twice (the new key and the
    /// key it collides with).
    #[cfg_attr(luna_hot, inline(always))]
    pub(super) fn main_position_inline(&self, k: Value) -> usize {
        match k {
            Value::Str(s) => s.hash() as usize & self.node_mask as usize,
            k => self.main_position(k),
        }
    }

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
            Value::Str(s) => s.hash() as usize & mask,
            // `hashpointer`: modulo, as numbers
            k => (u64::from(pointer_hash(k)) % odd) as usize,
        }
    }
}

impl Table {
    /// Put `k`, absent from the hash part, into it as PUC `luaH_newkey`
    /// does; `false`, having changed nothing but the free-node cursor, when
    /// no node is free. The hash part is not empty; `khash` is
    /// [`node::khash_of`] the key.
    #[cfg_attr(luna_hot, inline(always))]
    pub(super) fn place_new(&mut self, k: Value, v: Value, khash: u16) -> bool {
        let mp = match k {
            Value::Str(_) if self.node_mask <= KHASH_MASK => {
                (khash as u32 & self.node_mask) as usize
            }
            k => self.main_position_inline(k),
        };
        // a main position holding no value is taken over as PUC
        // `luaH_newkey` does: a free node, or one whose key was removed or
        // swept, which keeps its `next` so the chain through it stays whole
        let n = &self.nodes()[mp];
        if n.val.is_nil() && (n.is_free() || !reuse_by_address(n, &k)) {
            let next = n.next;
            self.nodes_mut()[mp] = Node::new(k, v, next, khash);
            return true;
        }
        self.place_colliding(mp, k, v, khash)
    }

    /// [`Self::place_new`] when the main position `mp` holds a value.
    #[inline(never)]
    fn place_colliding(&mut self, mp: usize, k: Value, v: Value, khash: u16) -> bool {
        let Some(free) = self.free_pos() else {
            return false;
        };
        let other = &self.nodes()[mp];
        let other_mp = if other.dead_key() {
            match other.dead_hash() {
                (h, true) => (h & self.node_mask) as usize,
                (h, false) => (u64::from(h) % u64::from(self.node_mask | 1)) as usize,
            }
        } else if other.key_tag == crate::runtime::value::tag::STR && self.node_mask <= KHASH_MASK {
            (other.khash as u32 & self.node_mask) as usize
        } else {
            self.main_position_inline(other.key())
        };
        if other_mp != mp {
            // colliding node is out of its main position: relocate it to the
            // free slot and take its place
            let mut prev = other_mp;
            while self.nodes()[prev].next != mp as i32 {
                prev = self.nodes()[prev].next as usize;
            }
            self.nodes_mut()[prev].next = free as i32;
            let moved = self.nodes()[mp];
            self.nodes_mut()[free] = moved;
            self.nodes_mut()[mp] = Node::new(k, v, NONE, khash);
        } else {
            // colliding node owns this position: chain the new node behind it
            let next = self.nodes()[mp].next;
            self.nodes_mut()[free] = Node::new(k, v, next, khash);
            self.nodes_mut()[mp].next = free as i32;
        }
        true
    }

    pub(super) fn free_pos(&mut self) -> Option<usize> {
        while self.lastfree > 0 {
            self.lastfree -= 1;
            let n = &self.nodes()[self.lastfree as usize];
            // Dead-key slots are still occupied for chain purposes (their
            // `next` may be the only path to a downstream entry) — don't
            // hand them out as free.
            if n.is_free() {
                return Some(self.lastfree as usize);
            }
        }
        None
    }
}

/// Whether taking over node `n` (no value, its key removed or dead) for
/// the new key `k` would depend on where the allocator put objects: the
/// new key or the removed one is hashed by its address, or the removed one
/// is a dead key (whose main position PUC takes from its address). Such a
/// node counts as taken, as if the addresses had not met and the
/// allocator had not given an address back: PUC's layout there changes
/// with the allocator, and luna does not model it.
#[inline]
fn reuse_by_address(n: &Node, k: &Value) -> bool {
    n.dead_key()
        || hashed_by_address(n.key_tag)
        || matches!(
            k,
            Value::Table(_)
                | Value::Closure(_)
                | Value::Native(_)
                | Value::Coro(_)
                | Value::Userdata(_)
                | Value::LightUserdata(_)
        )
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

/// What a key about to go dead hashed by, kept in its node: a string's hash
/// (`true`), or an object's number.
pub(super) fn dead_key_hash(k: Value) -> (u32, bool) {
    match k {
        Value::Str(s) => (s.hash(), true),
        k => (pointer_hash(k), false),
    }
}

/// Whether PUC hashes key `k` by its address (`hashpointer`).
#[cfg_attr(luna_hot, inline(always))]
pub(super) fn hashed_by_address(tag: u8) -> bool {
    use crate::runtime::value::tag;
    !matches!(tag, tag::NIL | tag::BOOL | tag::INT | tag::FLOAT | tag::STR)
}

/// What PUC's `hashpointer` takes of an object key: the low 32 bits of
/// its address; for luna's objects, the number each got when made
/// (`Heap::take_id`).
#[inline]
pub(super) fn pointer_hash(k: Value) -> u32 {
    match k {
        Value::Table(t) => t.id,
        Value::Closure(c) => c.hdr.aux,
        Value::Native(n) => n.hdr.aux,
        Value::Coro(co) => co.hdr.aux,
        Value::Userdata(u) => u.hdr.aux,
        Value::LightUserdata(p) => p as u32,
        // the keys `main_position` handles itself
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every live node is on the chain of its key's main position, worked
    /// out from the key itself, and a string key's node holds the low bits
    /// of its hash: below 2^16 nodes, where placement reads those bits, and
    /// past it, where it loads the string.
    #[test]
    fn string_keys_sit_on_their_main_position_chains() {
        for n in [5_000, 70_000] {
            crate::runtime::table::tests::with_table(|heap, t| {
                for i in 0..n {
                    let k = Value::Str(heap.intern(format!("k{i:05}").as_bytes()));
                    t.set(heap, k, Value::Int(i)).unwrap();
                }
                assert_eq!(t.nodes().len() > 1 << 16, n > 1 << 16);
                // every key is inserted, each placement reading its string
                // past 2^16 nodes; under Miri, where the whole check would
                // take most of an hour more, a fixed stride of the nodes and
                // keys is checked
                let stride = if cfg!(miri) { 61 } else { 1 };
                for (i, node) in t.nodes().iter().enumerate().step_by(stride) {
                    let Value::Str(s) = node.key() else { continue };
                    assert_eq!(node.khash, s.hash() as u16);
                    let mut at = t.main_position(node.key());
                    while at != i {
                        assert_ne!(t.nodes()[at].next, NONE, "node {i} off its chain");
                        at = t.nodes()[at].next as usize;
                    }
                }
                for i in (0..n).step_by(stride) {
                    let k = Value::Str(heap.intern(format!("k{i:05}").as_bytes()));
                    assert!(matches!(t.get(k), Value::Int(v) if v == i));
                }
            });
        }
    }

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
