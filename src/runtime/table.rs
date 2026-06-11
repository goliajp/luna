//! Lua table: hybrid array + hash.
//!
//! Array part uses split tag/payload storage (9 bytes/slot — the Lua 5.5
//! "compact arrays" layout, bench-validated in benches/value_repr.rs).
//! Hash part is the PUC node layout: main-position chaining with relocation
//! (Brent's variation), capacity a power of two, rehash sizing per
//! luaH_rehash/computesizes.

use crate::runtime::heap::{Gc, GcHeader};
use crate::runtime::value::{RawVal, Value, f2i_exact, raw};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableError {
    NilIndex,
    NanIndex,
    /// `next` called with a key not present in the table.
    InvalidNext,
}

#[derive(Clone, Copy)]
struct Node {
    key: Value,
    val: Value,
    /// absolute index of the next node in this chain, or NONE
    next: i32,
}

const NONE: i32 = -1;

impl Node {
    const EMPTY: Node = Node {
        key: Value::Nil,
        val: Value::Nil,
        next: NONE,
    };
}

pub struct Table {
    /// read through raw casts by the GC, not by field access
    #[allow(dead_code)]
    pub(crate) hdr: GcHeader,
    /// array part: parallel tag/payload slots for keys 1..=atags.len()
    atags: Box<[u8]>,
    avals: Box<[RawVal]>,
    /// hash part: power-of-two length (or empty)
    nodes: Box<[Node]>,
    /// free-slot search position, counts down (PUC lastfree)
    lastfree: u32,
    metatable: Option<Gc<Table>>,
    /// absent-metamethod cache (wired up in P03)
    #[allow(dead_code)]
    pub(crate) flags: u8,
}

impl Table {
    pub(crate) fn new(hdr: GcHeader) -> Table {
        Table {
            hdr,
            atags: Box::new([]),
            avals: Box::new([]),
            nodes: Box::new([]),
            lastfree: 0,
            metatable: None,
            flags: 0,
        }
    }

    pub fn metatable(&self) -> Option<Gc<Table>> {
        self.metatable
    }

    pub fn set_metatable(&mut self, mt: Option<Gc<Table>>) {
        self.metatable = mt;
    }

    fn asize(&self) -> usize {
        self.atags.len()
    }

    fn aget(&self, idx: usize) -> Value {
        unsafe { Value::pack(self.atags[idx], self.avals[idx]) }
    }

    fn aset(&mut self, idx: usize, v: Value) {
        let (t, b) = v.unpack();
        self.atags[idx] = t;
        self.avals[idx] = b;
    }

    // ---- reads ----

    pub fn get(&self, key: Value) -> Value {
        match key {
            Value::Int(i) => self.get_int(i),
            Value::Float(f) => match f2i_exact(f) {
                Some(i) => self.get_int(i),
                None => {
                    if f.is_nan() {
                        Value::Nil
                    } else {
                        self.get_hash(key)
                    }
                }
            },
            Value::Nil => Value::Nil,
            k => self.get_hash(k),
        }
    }

    pub fn get_int(&self, i: i64) -> Value {
        if i >= 1 && (i as u64) <= self.asize() as u64 {
            return self.aget(i as usize - 1);
        }
        self.get_hash(Value::Int(i))
    }

    fn get_hash(&self, k: Value) -> Value {
        match self.find_node(k) {
            Some(idx) => self.nodes[idx].val,
            None => Value::Nil,
        }
    }

    /// Walk the chain rooted at the key's main position.
    fn find_node(&self, k: Value) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut idx = self.main_position(k);
        loop {
            let n = &self.nodes[idx];
            if n.key.raw_eq(k) {
                return Some(idx);
            }
            if n.next == NONE {
                return None;
            }
            idx = n.next as usize;
        }
    }

    // ---- writes ----

    pub fn set(&mut self, key: Value, val: Value) -> Result<(), TableError> {
        let k = normalize_set_key(key)?;
        self.set_norm(k, val);
        Ok(())
    }

    pub fn set_int(&mut self, i: i64, val: Value) {
        self.set_norm(Value::Int(i), val);
    }

    /// `k` is already normalized (no nil, no NaN, integral floats → Int).
    fn set_norm(&mut self, k: Value, v: Value) {
        if let Value::Int(i) = k
            && i >= 1
            && (i as u64) <= self.asize() as u64
        {
            self.aset(i as usize - 1, v);
            return;
        }
        if let Some(idx) = self.find_node(k) {
            self.nodes[idx].val = v;
            return;
        }
        if v.is_nil() {
            return; // absent key set to nil: nothing to record
        }
        self.insert_new(k, v);
    }

    fn insert_new(&mut self, k: Value, v: Value) {
        if self.nodes.is_empty() {
            self.rehash(k);
            self.set_norm(k, v);
            return;
        }
        let mp = self.main_position(k);
        if self.nodes[mp].key.is_nil() {
            self.nodes[mp] = Node {
                key: k,
                val: v,
                next: NONE,
            };
            return;
        }
        let Some(free) = self.free_pos() else {
            self.rehash(k);
            self.set_norm(k, v);
            return;
        };
        let other_mp = self.main_position(self.nodes[mp].key);
        if other_mp != mp {
            // colliding node is out of its main position: relocate it to the
            // free slot and take its place
            let mut prev = other_mp;
            while self.nodes[prev].next != mp as i32 {
                prev = self.nodes[prev].next as usize;
            }
            self.nodes[prev].next = free as i32;
            self.nodes[free] = self.nodes[mp];
            self.nodes[mp] = Node {
                key: k,
                val: v,
                next: NONE,
            };
        } else {
            // colliding node owns this position: chain the new node behind it
            self.nodes[free] = Node {
                key: k,
                val: v,
                next: self.nodes[mp].next,
            };
            self.nodes[mp].next = free as i32;
        }
    }

    fn free_pos(&mut self) -> Option<usize> {
        while self.lastfree > 0 {
            self.lastfree -= 1;
            if self.nodes[self.lastfree as usize].key.is_nil() {
                return Some(self.lastfree as usize);
            }
        }
        None
    }

    // ---- rehash (PUC luaH_rehash) ----

    fn rehash(&mut self, pending: Value) {
        let mut nums = [0usize; 65];
        let mut int_keys = 0usize;
        let mut total = 1; // the pending key
        if let Value::Int(i) = pending
            && i >= 1
        {
            nums[ceil_log2(i as u64)] += 1;
            int_keys += 1;
        }
        for i in 0..self.asize() {
            if self.atags[i] != raw::NIL {
                nums[ceil_log2(i as u64 + 1)] += 1;
                int_keys += 1;
                total += 1;
            }
        }
        for n in self.nodes.iter() {
            if !n.val.is_nil() {
                total += 1;
                if let Value::Int(i) = n.key
                    && i >= 1
                {
                    nums[ceil_log2(i as u64)] += 1;
                    int_keys += 1;
                }
            }
        }
        // computesizes: optimal array size = largest 2^i with more than 2^(i-1)
        // integer keys in [1, 2^i]
        let mut new_asize = 0usize;
        let mut in_array = 0usize;
        let mut a = 0usize;
        let mut two_to_i = 1usize;
        let mut i = 0usize;
        while int_keys > two_to_i / 2 {
            a += nums[i];
            if a > two_to_i / 2 {
                new_asize = two_to_i;
                in_array = a;
            }
            i += 1;
            match two_to_i.checked_mul(2) {
                Some(n) => two_to_i = n,
                None => break,
            }
        }
        self.resize(new_asize, total - in_array);
    }

    fn resize(&mut self, new_asize: usize, hash_entries: usize) {
        let old_atags = std::mem::take(&mut self.atags);
        let old_avals = std::mem::take(&mut self.avals);
        let old_nodes = std::mem::take(&mut self.nodes);
        self.atags = vec![raw::NIL; new_asize].into_boxed_slice();
        self.avals = vec![RawVal::NIL; new_asize].into_boxed_slice();
        let hsize = if hash_entries == 0 {
            0
        } else {
            hash_entries.next_power_of_two()
        };
        self.nodes = vec![Node::EMPTY; hsize].into_boxed_slice();
        self.lastfree = hsize as u32;
        for (i, &tag) in old_atags.iter().enumerate() {
            if tag != raw::NIL {
                let v = unsafe { Value::pack(tag, old_avals[i]) };
                self.set_norm(Value::Int(i as i64 + 1), v);
            }
        }
        for n in old_nodes.iter() {
            if !n.val.is_nil() {
                self.set_norm(n.key, n.val);
            }
        }
    }

    fn main_position(&self, k: Value) -> usize {
        debug_assert!(!self.nodes.is_empty());
        hash_key(k) as usize & (self.nodes.len() - 1)
    }

    // ---- length / iteration ----

    /// A border: n where t[n] is non-nil and t[n+1] is nil (PUC luaH_getn).
    /// This is Lua `#` semantics, not a container size — an `is_empty`
    /// counterpart would be meaningless.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> i64 {
        let asize = self.asize();
        if asize > 0 && self.atags[asize - 1] == raw::NIL {
            // binary search inside the array part
            let (mut lo, mut hi) = (0usize, asize);
            while hi - lo > 1 {
                let m = lo + (hi - lo) / 2;
                if self.atags[m - 1] == raw::NIL {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            return lo as i64;
        }
        if self.nodes.is_empty() {
            return asize as i64;
        }
        // array is full (or absent): unbound search through the hash part
        let mut lo = asize as i64;
        let mut hi = lo + 1;
        while !self.get_int(hi).is_nil() {
            lo = hi;
            match hi.checked_mul(2) {
                Some(n) => hi = n,
                None => {
                    // pathological: linear scan
                    let mut i = lo + 1;
                    while !self.get_int(i).is_nil() {
                        i += 1;
                    }
                    return i - 1;
                }
            }
        }
        while hi - lo > 1 {
            let m = lo + (hi - lo) / 2;
            if self.get_int(m).is_nil() {
                hi = m;
            } else {
                lo = m;
            }
        }
        lo
    }

    /// Lua `next`: iterate array part then hash part.
    pub fn next(&self, key: Value) -> Result<Option<(Value, Value)>, TableError> {
        let start = match key {
            Value::Nil => 0,
            k => {
                let k = match k {
                    Value::Float(f) => match f2i_exact(f) {
                        Some(i) => Value::Int(i),
                        None => k,
                    },
                    k => k,
                };
                if let Value::Int(i) = k
                    && i >= 1
                    && (i as u64) <= self.asize() as u64
                {
                    i as usize
                } else {
                    match self.find_node(k) {
                        Some(idx) => self.asize() + idx + 1,
                        None => return Err(TableError::InvalidNext),
                    }
                }
            }
        };
        for i in start..self.asize() {
            if self.atags[i] != raw::NIL {
                return Ok(Some((Value::Int(i as i64 + 1), self.aget(i))));
            }
        }
        let hstart = start.saturating_sub(self.asize());
        for (idx, n) in self.nodes.iter().enumerate().skip(hstart) {
            if !n.val.is_nil() {
                let _ = idx;
                return Ok(Some((n.key, n.val)));
            }
        }
        Ok(None)
    }

    /// GC: visit every contained reference (P02 traces keys conservatively;
    /// dead-key handling arrives with weak tables in P06).
    pub(crate) fn trace(&self, mark: &mut dyn FnMut(Value)) {
        for (i, &tag) in self.atags.iter().enumerate() {
            if raw::is_gc(tag) {
                mark(unsafe { Value::pack(tag, self.avals[i]) });
            }
        }
        for n in self.nodes.iter() {
            mark(n.key);
            mark(n.val);
        }
        if let Some(mt) = self.metatable {
            mark(Value::Table(mt));
        }
    }
}

fn normalize_set_key(key: Value) -> Result<Value, TableError> {
    match key {
        Value::Nil => Err(TableError::NilIndex),
        Value::Float(f) => match f2i_exact(f) {
            Some(i) => Ok(Value::Int(i)),
            None if f.is_nan() => Err(TableError::NanIndex),
            None => Ok(key),
        },
        k => Ok(k),
    }
}

fn hash_key(k: Value) -> u64 {
    match k {
        Value::Int(i) => i as u64, // identity mod size (PUC hashint)
        Value::Float(f) => mix64(f.to_bits()),
        Value::Bool(b) => b as u64 + 1,
        Value::Str(s) => s.hash() as u64,
        Value::Table(t) => mix64(t.as_ptr() as u64),
        Value::Nil => 0, // unreachable as a stored key
    }
}

/// splitmix64 finalizer.
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// For k ≥ 1: the bucket l such that k ∈ (2^(l-1), 2^l].
fn ceil_log2(k: u64) -> usize {
    (u64::BITS - (k - 1).leading_zeros()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::heap::Heap;

    fn with_table(f: impl FnOnce(&mut Heap, &mut Table)) {
        let mut heap = Heap::new();
        let t = heap.new_table();
        f(&mut heap, unsafe { t.as_mut() });
    }

    fn assert_is_border(t: &Table, n: i64) {
        if n == 0 {
            assert!(t.get_int(1).is_nil(), "border 0 but t[1] non-nil");
        } else {
            assert!(!t.get_int(n).is_nil(), "border {n} but t[{n}] is nil");
            assert!(
                t.get_int(n + 1).is_nil(),
                "border {n} but t[{}] non-nil",
                n + 1
            );
        }
    }

    #[test]
    fn sequence_grows_into_array() {
        with_table(|_, t| {
            for i in 1..=1000 {
                t.set_int(i, Value::Int(i * 10));
            }
            for i in 1..=1000 {
                assert!(t.get_int(i).raw_eq(Value::Int(i * 10)));
            }
            assert_eq!(t.len(), 1000);
        });
    }

    #[test]
    fn string_and_mixed_keys() {
        with_table(|heap, t| {
            let k1 = Value::Str(heap.intern(b"alpha"));
            let k2 = Value::Str(heap.intern(b"beta"));
            t.set(k1, Value::Int(1)).unwrap();
            t.set(k2, Value::Int(2)).unwrap();
            t.set(Value::Bool(true), Value::Int(3)).unwrap();
            t.set(Value::Int(-5), Value::Int(4)).unwrap();
            // re-interned key reaches the same slot
            let k1b = Value::Str(heap.intern(b"alpha"));
            assert!(t.get(k1b).raw_eq(Value::Int(1)));
            assert!(t.get(k2).raw_eq(Value::Int(2)));
            assert!(t.get(Value::Bool(true)).raw_eq(Value::Int(3)));
            assert!(t.get(Value::Int(-5)).raw_eq(Value::Int(4)));
            assert!(t.get(Value::Str(heap.intern(b"gamma"))).is_nil());
        });
    }

    #[test]
    fn float_keys_normalize_to_int() {
        with_table(|_, t| {
            t.set(Value::Float(2.0), Value::Int(22)).unwrap();
            assert!(t.get(Value::Int(2)).raw_eq(Value::Int(22)));
            t.set(Value::Int(3), Value::Int(33)).unwrap();
            assert!(t.get(Value::Float(3.0)).raw_eq(Value::Int(33)));
            // -0.0 is key 0
            t.set(Value::Float(-0.0), Value::Int(0)).unwrap();
            assert!(t.get(Value::Int(0)).raw_eq(Value::Int(0)));
            // non-integral floats are their own keys
            t.set(Value::Float(0.5), Value::Int(55)).unwrap();
            assert!(t.get(Value::Float(0.5)).raw_eq(Value::Int(55)));
            assert!(t.get(Value::Int(0)).raw_eq(Value::Int(0)));
        });
    }

    #[test]
    fn bad_keys() {
        with_table(|_, t| {
            assert_eq!(t.set(Value::Nil, Value::Int(1)), Err(TableError::NilIndex));
            assert_eq!(
                t.set(Value::Float(f64::NAN), Value::Int(1)),
                Err(TableError::NanIndex)
            );
            // reads with bad keys are nil, not errors
            assert!(t.get(Value::Nil).is_nil());
            assert!(t.get(Value::Float(f64::NAN)).is_nil());
        });
    }

    #[test]
    fn delete_and_reinsert() {
        with_table(|heap, t| {
            let k = Value::Str(heap.intern(b"k"));
            t.set(k, Value::Int(1)).unwrap();
            t.set(k, Value::Nil).unwrap();
            assert!(t.get(k).is_nil());
            t.set(k, Value::Int(2)).unwrap();
            assert!(t.get(k).raw_eq(Value::Int(2)));
            // setting an absent key to nil stays absent
            let k2 = Value::Str(heap.intern(b"k2"));
            t.set(k2, Value::Nil).unwrap();
            assert!(t.get(k2).is_nil());
        });
    }

    #[test]
    fn borders_with_holes() {
        with_table(|_, t| {
            t.set_int(1, Value::Int(1));
            t.set_int(2, Value::Int(2));
            assert_eq!(t.len(), 2);
            t.set_int(2, Value::Nil);
            assert_is_border(t, t.len());
            // hash-resident tail
            t.set_int(1_000_000, Value::Int(1));
            assert_is_border(t, t.len());
        });
    }

    #[test]
    fn len_on_empty_and_hash_only() {
        with_table(|heap, t| {
            assert_eq!(t.len(), 0);
            t.set(Value::Str(heap.intern(b"x")), Value::Int(1)).unwrap();
            assert_eq!(t.len(), 0);
        });
    }

    #[test]
    fn next_iterates_everything_exactly_once() {
        with_table(|heap, t| {
            let mut expected = 0i64;
            for i in 1..=64 {
                t.set_int(i, Value::Int(i));
                expected += i;
            }
            for i in 0..32 {
                let k = Value::Str(heap.intern(format!("s{i}").as_bytes()));
                t.set(k, Value::Int(1000 + i)).unwrap();
                expected += 1000 + i;
            }
            t.set(Value::Float(2.5), Value::Int(7)).unwrap();
            expected += 7;

            let mut sum = 0i64;
            let mut count = 0;
            let mut key = Value::Nil;
            while let Some((k, v)) = t.next(key).unwrap() {
                let Value::Int(x) = v else {
                    panic!("bad value")
                };
                sum += x;
                count += 1;
                key = k;
            }
            assert_eq!(count, 64 + 32 + 1);
            assert_eq!(sum, expected);
        });
    }

    #[test]
    fn next_skips_nil_values_and_rejects_alien_keys() {
        with_table(|heap, t| {
            t.set_int(1, Value::Int(1));
            t.set_int(3, Value::Int(3));
            let k = Value::Str(heap.intern(b"gone"));
            t.set(k, Value::Int(9)).unwrap();
            t.set(k, Value::Nil).unwrap();
            let mut seen = Vec::new();
            let mut key = Value::Nil;
            while let Some((k, v)) = t.next(key).unwrap() {
                let Value::Int(x) = v else { panic!() };
                seen.push(x);
                key = k;
            }
            assert_eq!(seen, vec![1, 3]);
            // a key never inserted is invalid for next
            let alien = Value::Str(heap.intern(b"never"));
            assert!(matches!(t.next(alien), Err(TableError::InvalidNext)));
            // ...but a deleted (nil-valued) key is still a valid cursor
            assert!(t.next(k).is_ok());
        });
    }

    #[test]
    fn collision_relocation_keeps_chains_intact() {
        with_table(|_, t| {
            // dense negative ints all land in the hash part; with identity
            // hashing they exercise both chain cases heavily
            for i in 0..512 {
                t.set_int(-i, Value::Int(i));
            }
            for i in 0..512 {
                assert!(t.get_int(-i).raw_eq(Value::Int(i)), "lost key {}", -i);
            }
        });
    }

    #[test]
    fn rehash_redistributes_into_array() {
        with_table(|_, t| {
            // insert 1..n in reverse: starts in hash, rehash must migrate
            for i in (1..=256).rev() {
                t.set_int(i, Value::Int(i));
            }
            assert_eq!(t.len(), 256);
            for i in 1..=256 {
                assert!(t.get_int(i).raw_eq(Value::Int(i)));
            }
        });
    }
}
