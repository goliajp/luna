//! `LMap<K, V>`: a hash map whose table comes from a [`MemCtx`].

use std::hash::{Hash, Hasher};

use super::ctx::{MemRef, Oom};
use super::vec::LVec;

/// A multiply-rotate hasher over the key's words: the keys the runtime maps
/// are a few numbers or pointers, where SipHash's setup cost dominates.
#[derive(Default)]
pub struct WordHasher(u64);

impl Hasher for WordHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(u64::from(v));
    }
    fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
    fn write_isize(&mut self, v: isize) {
        self.write_u64(v as u64);
    }
}

/// The [`WordHasher`] hash of `k`.
pub fn word_hash<K: Hash + ?Sized>(k: &K) -> u64 {
    let mut h = WordHasher::default();
    k.hash(&mut h);
    h.finish()
}

/// A map of `Copy` keys and values with open addressing (linear probing)
/// over a power-of-two table at most half full. Inserting can fail
/// ([`Oom`]) and then leaves the map as it was. Entries are not removed
/// one by one, only all at once ([`LMap::clear`]).
///
/// Besides `Hash`-keyed access, a caller may supply its own hash and
/// equality ([`LMap::find_with`], [`LMap::insert_hashed`]), so keys that
/// stand for data held elsewhere (a string object for its bytes) can be
/// looked up by that data.
pub struct LMap<K: Copy, V: Copy> {
    /// `(hash, key, value)` or `None`
    slots: LVec<Option<(u64, K, V)>>,
    len: usize,
}

impl<K: Copy, V: Copy> LMap<K, V> {
    /// An empty map; allocates nothing.
    pub fn new(mem: MemRef) -> LMap<K, V> {
        LMap {
            slots: LVec::new(mem),
            len: 0,
        }
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Drop every entry, keeping the table.
    pub fn clear(&mut self) {
        for s in self.slots.iter_mut() {
            *s = None;
        }
        self.len = 0;
    }

    /// The value of the entry with hash `h` whose key `eq` accepts.
    pub fn find_with(&self, h: u64, mut eq: impl FnMut(&K) -> bool) -> Option<V> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut i = h as usize & mask;
        loop {
            match &self.slots[i] {
                None => return None,
                Some((sh, k, v)) if *sh == h && eq(k) => return Some(*v),
                Some(_) => i = (i + 1) & mask,
            }
        }
    }

    /// Add an entry with hash `h` the caller knows is not in the map.
    pub fn insert_hashed(&mut self, h: u64, k: K, v: V) -> Result<(), Oom> {
        if (self.len + 1) * 2 > self.slots.len() {
            self.grow()?;
        }
        Self::place(&mut self.slots, h, k, v);
        self.len += 1;
        Ok(())
    }

    fn place(slots: &mut [Option<(u64, K, V)>], h: u64, k: K, v: V) {
        let mask = slots.len() - 1;
        let mut i = h as usize & mask;
        while slots[i].is_some() {
            i = (i + 1) & mask;
        }
        slots[i] = Some((h, k, v));
    }

    #[cold]
    fn grow(&mut self) -> Result<(), Oom> {
        let n = (self.slots.len() * 2).max(8);
        let mut slots = LVec::with_capacity(self.slots.mem(), n)?;
        slots.resize(n, None)?;
        for &(h, k, v) in self.slots.iter().flatten() {
            Self::place(&mut slots, h, k, v);
        }
        self.slots = slots;
        Ok(())
    }

    /// Room for `n` entries in all without growing.
    pub fn reserve(&mut self, n: usize) -> Result<(), Oom> {
        while n * 2 > self.slots.len() {
            self.grow()?;
        }
        Ok(())
    }

    /// The entries, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (K, V)> + '_ {
        self.slots.iter().flatten().map(|&(_, k, v)| (k, v))
    }
}

impl<K: Copy + Eq + Hash, V: Copy> LMap<K, V> {
    /// The value of `k`.
    pub fn get(&self, k: &K) -> Option<V> {
        self.find_with(word_hash(k), |x| x == k)
    }

    /// [`LMap::insert`] with no memory error to return: see
    /// `oom_abort`.
    pub fn insert_or_abort(&mut self, k: K, v: V) -> Option<V> {
        self.insert(k, v).unwrap_or_else(|o| o.fail())
    }

    /// Set `k` to `v`, returning the value it replaced.
    pub fn insert(&mut self, k: K, v: V) -> Result<Option<V>, Oom> {
        let h = word_hash(&k);
        if !self.slots.is_empty() {
            let mask = self.slots.len() - 1;
            let mut i = h as usize & mask;
            while let Some((sh, sk, sv)) = &mut self.slots[i] {
                if *sh == h && *sk == k {
                    return Ok(Some(std::mem::replace(sv, v)));
                }
                i = (i + 1) & mask;
            }
        }
        self.insert_hashed(h, k, v)?;
        Ok(None)
    }
}
