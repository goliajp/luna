//! The parser's tables as counts: which keys each holds and how big PUC's
//! array and hash parts are, recorded as PUC's table (`GhostRef`).
//!
//! A parser table only gains keys (5.5's chunk table loses its functions'
//! tables, whose nodes stay occupied), so PUC rehashes exactly when a new
//! key belongs in a hash part with no free node, whatever the hashes: the
//! sizes follow from how many keys there are and which are integers.

use super::*;
use crate::runtime::heap::GhostRef;

/// A key's identity in a parser table.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PKey {
    Int(i64),
    Float(u64),
    Bool(bool),
    /// a string, object or light userdata, by address
    Ptr(usize),
    /// another parser table, by number
    Tab(u32),
}

impl PKey {
    /// The key PUC's table has for `v` (an integral float is the integer).
    pub(super) fn of(v: Value) -> PKey {
        match v {
            Value::Int(i) => PKey::Int(i),
            Value::Float(f) => match crate::runtime::value::f2i_exact(f) {
                Some(i) => PKey::Int(i),
                None => PKey::Float(f.to_bits()),
            },
            Value::Bool(b) => PKey::Bool(b),
            Value::Str(s) => PKey::Ptr(s.as_ptr() as usize),
            Value::LightUserdata(p) => PKey::Ptr(p as usize),
            Value::Table(t) => PKey::Ptr(t.as_ptr() as usize),
            Value::Closure(c) => PKey::Ptr(c.as_ptr() as usize),
            _ => PKey::Ptr(0),
        }
    }

    /// The key of string `s` (a long string by its bytes, the first of
    /// them keyed standing for all, in `longs`).
    pub(super) fn of_str(
        s: Gc<crate::runtime::LuaStr>,
        longs: &mut std::collections::HashMap<Box<[u8]>, usize>,
    ) -> PKey {
        let p = s.as_ptr() as usize;
        if s.is_short() {
            return PKey::Ptr(p);
        }
        PKey::Ptr(*longs.entry(s.as_bytes().into()).or_insert(p))
    }

    /// The integer of a key that may go to the array part.
    fn index(self) -> Option<u64> {
        match self {
            PKey::Int(i) if i >= 1 => Some(i as u64),
            _ => None,
        }
    }
}

/// A key hasher for [`PTab`]'s set: the keys are distinct addresses and
/// small numbers.
#[derive(Default)]
pub(super) struct KeyHasher(u64);

impl std::hash::Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x51_7cc1_b727_220a_95);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7cc1_b727_220a_95);
    }
    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}

/// One of the parser's tables.
pub(super) struct PTab {
    pub(super) id: u32,
    keys: std::collections::HashSet<PKey, std::hash::BuildHasherDefault<KeyHasher>>,
    /// PUC's array size and node count, and the nodes taken
    asize: u64,
    nodes: u64,
    used: u64,
    /// a key was removed (its node stays taken)
    removed: bool,
    /// the keys that may go to the array part, by `ceil_log2`
    nums: [u32; 65],
    ghost: Option<GhostRef>,
}

impl PTab {
    /// A new empty table (PUC `luaH_new`), counted as PUC's.
    pub(super) fn new(heap: &mut Heap, id: u32) -> Result<PTab, SyntaxError> {
        Ok(PTab {
            id,
            keys: Default::default(),
            asize: 0,
            nodes: 0,
            used: 0,
            removed: false,
            nums: [0; 65],
            ghost: Some(heap.acct_ghost_table(0, 0)?),
        })
    }

    #[cfg(feature = "gc-trace")]
    pub(super) fn contains(&self, k: PKey) -> bool {
        self.keys.contains(&k)
    }

    /// `k` put in the table (PUC `luaH_newkey`), when it is not there.
    pub(super) fn insert(&mut self, heap: &mut Heap, d: Dialect, k: PKey) {
        if self.keys.contains(&k) {
            return;
        }
        let in_array = k.index().is_some_and(|i| i <= self.asize);
        if !in_array && self.used == self.nodes {
            self.rehash(heap, d, k);
        } else if !in_array {
            self.used += 1;
        }
        if let Some(i) = k.index() {
            self.nums[ceil_log2(i)] += 1;
        }
        self.keys.insert(k);
    }

    /// `k` set to nil: the key goes, its node stays taken.
    pub(super) fn remove(&mut self, k: PKey) {
        if self.keys.remove(&k) {
            if let Some(i) = k.index() {
                self.nums[ceil_log2(i)] -= 1;
            }
            self.removed = true;
        }
    }

    /// PUC `rehash` with `pending` the key that found no room, then the key
    /// placed: the parts resized as `luaH_resize` does.
    fn rehash(&mut self, heap: &mut Heap, d: Dialect, pending: PKey) {
        let mut nums = self.nums;
        if let Some(i) = pending.index() {
            nums[ceil_log2(i)] += 1;
        }
        let total = self.keys.len() as u64 + 1;
        let (asize, in_array) = match d {
            Dialect::L55 => sizes_55(&nums, self.array_keys(), self.asize),
            _ => sizes(&nums),
        };
        let mut hash = total - in_array;
        if d == Dialect::L55 && (self.removed || self.nodes == 0) {
            hash += hash >> 2;
        }
        let nodes = if hash == 0 {
            0
        } else {
            hash.next_power_of_two()
        };
        if let Some(g) = self.ghost {
            heap.ghost_table_resize(
                g,
                self.asize as usize,
                self.nodes as usize,
                asize as usize,
                nodes as usize,
            );
        }
        let in_hash = total - in_array;
        (self.asize, self.nodes, self.used, self.removed) = (asize, nodes, in_hash, false);
    }

    /// The keys in the array part (its size is a power of two, or 0).
    fn array_keys(&self) -> u64 {
        (0..65)
            .take_while(|&b| 1u64 << b <= self.asize)
            .map(|b| u64::from(self.nums[b]))
            .sum()
    }
}

/// The bucket of integer key `k` (PUC `luaO_ceillog2`).
fn ceil_log2(k: u64) -> usize {
    (u64::BITS - (k - 1).leading_zeros()) as usize
}

/// 5.1–5.4 `computesizes`: the largest power of two with more than half
/// its slots taken by keys, and how many keys go there.
fn sizes(nums: &[u32; 65]) -> (u64, u64) {
    let int_keys: u64 = nums.iter().map(|&n| u64::from(n)).sum();
    let (mut a, mut asize, mut in_array) = (0u64, 0u64, 0u64);
    let mut two_to_i = 1u64;
    let mut i = 0;
    while int_keys > two_to_i / 2 && i < 65 {
        a += u64::from(nums[i]);
        if a > two_to_i / 2 {
            asize = two_to_i;
            in_array = a;
        }
        i += 1;
        two_to_i *= 2;
    }
    (asize, in_array)
}

/// 5.5 `rehash`: with no integer key outside the array part (`arr` keys
/// there), the array part stays; otherwise the largest power of two
/// holding more keys in its last slice and no more than three times as
/// many slots as keys.
fn sizes_55(nums: &[u32; 65], arr: u64, asize: u64) -> (u64, u64) {
    let na: u64 = nums.iter().map(|&n| u64::from(n)).sum();
    if na == arr {
        return (asize, arr);
    }
    let (mut a, mut optimal, mut in_array) = (0u64, 0u64, 0u64);
    let mut twotoi = 1u64;
    for &n in nums.iter() {
        if twotoi > 3 * na {
            break;
        }
        a += u64::from(n);
        if n > 0 && twotoi <= 3 * a {
            optimal = twotoi;
            in_array = a;
        }
        twotoi *= 2;
    }
    (optimal, in_array)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key of `keys` put in a real table and in a [`PTab`] of
    /// dialect `d` (`remove` keys set to nil again after them): the two
    /// have the same parts after each.
    fn same_parts(d: Dialect, keys: &[Value], remove: &[Value]) {
        let mut heap = Heap::new();
        heap.table_dialect = d;
        let t = heap.try_new_table().expect("table");
        let mut p = PTab::new(&mut heap, 1).expect("parser table");
        let check = |heap: &mut Heap, k: Value, set: bool, p: &mut PTab| {
            // SAFETY: the test's table, which nothing else refers to
            let t = unsafe { t.as_mut() };
            if set {
                t.set(heap, k, Value::Int(0)).expect("set");
                p.insert(heap, d, PKey::of(k));
            } else {
                t.set(heap, k, Value::Nil).expect("clear");
                p.remove(PKey::of(k));
            }
            let (a, n) = t.part_sizes();
            assert_eq!(
                (p.asize, p.nodes),
                (a as u64, n as u64),
                "{d:?} after {k:?}"
            );
        };
        for &k in keys {
            check(&mut heap, k, true, &mut p);
        }
        for &k in remove {
            check(&mut heap, k, false, &mut p);
        }
        for &k in keys.iter().rev().take(40) {
            check(&mut heap, k, true, &mut p);
        }
    }

    #[test]
    fn parts_grow_as_a_real_tables_do() {
        let dialects = [
            Dialect::L51,
            Dialect::L52,
            Dialect::L53,
            Dialect::L54,
            Dialect::L55,
        ];
        for d in dialects {
            let mut heap = Heap::new();
            let strs: Vec<Value> = (0..300)
                .map(|i| Value::Str(heap.try_intern(format!("k{i}").as_bytes()).expect("str")))
                .collect();
            let tabs: Vec<Value> = (0..60)
                .map(|_| Value::Table(heap.try_new_table().expect("t")))
                .collect();
            let ints: Vec<Value> = (1..=300).map(Value::Int).collect();
            let down: Vec<Value> = (1..=300).rev().map(Value::Int).collect();
            let gaps: Vec<Value> = (1..=200).map(|i| Value::Int(i * 7)).collect();
            let mixed: Vec<Value> = (0..300)
                .map(|i| match i % 4 {
                    0 => strs[i as usize],
                    1 => Value::Int(i / 2),
                    2 => Value::Float(i as f64 + 0.5),
                    _ => Value::Float((i / 3) as f64),
                })
                .collect();
            same_parts(d, &strs, &[]);
            same_parts(d, &ints, &[]);
            same_parts(d, &down, &[]);
            same_parts(d, &gaps, &[]);
            same_parts(d, &mixed, &[]);
            same_parts(d, &tabs, &tabs[10..30]);
        }
    }
}
