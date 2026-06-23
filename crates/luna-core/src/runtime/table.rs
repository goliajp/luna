//! Lua table: hybrid array + hash.
//!
//! Array part uses split tag/payload storage (9 bytes/slot — the Lua 5.5
//! "compact arrays" layout, bench-validated in benches/value_repr.rs).
//! Hash part is the PUC node layout: main-position chaining with relocation
//! (Brent's variation), capacity a power of two, rehash sizing per
//! luaH_rehash/computesizes.

use crate::runtime::heap::{Gc, GcHeader, Heap, Marker};
use crate::runtime::value::{RawVal, Value, f2i_exact, raw};

/// Errors that table mutation can raise back to the interpreter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableError {
    /// `t[nil] = …` — `nil` is forbidden as a key.
    NilIndex,
    /// `t[0/0] = …` — NaN floats are forbidden as keys.
    NanIndex,
    /// `next` called with a key not present in the table.
    InvalidNext,
    /// PUC `luaH_resizearray` — the array part would have to grow past
    /// `MAXASIZE`, or the hash part past `MAXHBITS`. Raised back as
    /// "table overflow" so a runaway `a[i] = i` loop walls within budget
    /// (5.5/5.4 heavy.lua's `toomanyidx` pcalls exactly this scenario).
    Overflow,
}

/// PUC `MAXASIZE` analogue: the highest power of two an array part may
/// grow to. Choose a cap that comfortably fits in the gate's 60-second
/// budget (each grow is O(n), so 2^27 entries × 16 bytes ≈ 2 GB is the
/// effective ceiling). Beyond this `rehash` returns `TableError::Overflow`.
pub(crate) const MAX_ASIZE: usize = 1 << 27;

#[derive(Clone, Copy)]
pub(crate) struct Node {
    key: Value,
    val: Value,
    /// absolute index of the next node in this chain, or NONE
    next: i32,
    /// PUC `setdeadkey` analogue: the key was a collectable that got swept
    /// out of a weak table. The Gc pointer in `key` is now dangling — its
    /// memory may have been reused for a new allocation with potentially
    /// equal content. Marking the node "dead-key" lets `find_node` skip the
    /// raw_eq probe (which could spuriously match a reallocated object) and
    /// `insert_new` treat the slot as available for a fresh main-position
    /// owner while leaving chain back-links intact for traversal.
    dead_key: bool,
}

const NONE: i32 = -1;

impl Node {
    const EMPTY: Node = Node {
        key: Value::Nil,
        val: Value::Nil,
        next: NONE,
        dead_key: false,
    };
}

/// P11-S5d.I — inline storage threshold. Tables whose array part has
/// `asize <= INLINE_ASIZE` keep their atags+avals inside the Table
/// struct itself (`inline_storage`), skipping the slab Box entirely
/// — binary_trees's `{nil, nil}` and `{...}` 2-element leaves live
/// here, sparing one allocator round-trip per NewTable.
pub(crate) const INLINE_ASIZE: u64 = 2;
/// `INLINE_ASIZE` u64 slots for avals + `ceil(INLINE_ASIZE / 8)` u64
/// slots covering the atags bytes (with trailing pad). For
/// `INLINE_ASIZE = 2`: 2 avals + 1 atags = 3 u64s = 24 bytes.
pub(crate) const INLINE_U64S: usize = INLINE_ASIZE as usize + INLINE_ASIZE.div_ceil(8) as usize;

/// Lua table — hybrid array + hash storage, with optional metatable and
/// weak-mode flags.
#[repr(C)]
pub struct Table {
    /// read through raw casts by the GC, not by field access
    #[allow(dead_code)]
    pub(crate) hdr: GcHeader,
    /// P11-S5d.I — single backing pointer for the array part. Points
    /// to `inline_storage` (asize <= INLINE_ASIZE) or `slab.as_ptr()`
    /// (asize > INLINE_ASIZE). The JIT inline aset reads this with one
    /// `load i64`, no branch — the choice between inline and slab is
    /// already encoded in the pointer. Initialised in `Heap::new_table`
    /// AFTER the Table reaches its final heap address (so that
    /// `&mut self.inline_storage` is the stable heap pointer, not a
    /// stack-local one). Updated by `Table::resize`.
    pub array_ptr: *mut u8,
    /// P11-S5d.H — external backing for the array part when
    /// `asize > INLINE_ASIZE`. Layout: `[avals: asize × 8 bytes][atags:
    /// asize bytes]`. Empty box (dangling, no alloc) when the inline
    /// path is in use.
    pub(crate) slab: Box<[u64]>,
    /// Length of the array part in slots. u64 (rather than `usize` or
    /// `u32`) so the JIT can load it with a single `load i64`.
    pub asize: u64,
    /// P11-S5d.I — inline backing used when `asize <= INLINE_ASIZE`.
    /// Same layout as the slab: avals at low addresses (`asize * 8`
    /// bytes from offset 0), atags at the trailing `asize` bytes.
    pub(crate) inline_storage: [u64; INLINE_U64S],
    /// hash part: power-of-two length (or empty)
    /// hash part: power-of-two length (or empty)
    /// `pub(crate)` so `Heap::free_obj` (pool recycle path) can reset.
    pub(crate) nodes: Box<[Node]>,
    /// free-slot search position, counts down (PUC lastfree).
    /// `pub(crate)` so `Heap::new_table` can reset on pool recycle.
    pub(crate) lastfree: u32,
    /// P11-S5d.K — visibility lifted to `pub(crate)` so the JIT can
    /// take its field offset at compile time and emit an inline
    /// "metatable.is_none()" guard before the inline aget fast path.
    /// `Option<Gc<Table>>` is 8 bytes via the NonNull-pointer-opt: 0
    /// ⇔ None, non-zero ⇔ Some.
    pub metatable: Option<Gc<Table>>,
    /// reserved for an absent-metamethod cache (PUC `flags`); currently
    /// unread — luna's mm lookup walks `metatable.get` each time
    #[allow(dead_code)]
    pub(crate) flags: u8,
}

// SAFETY: `array_ptr` looks like an unprotected raw pointer field, but
// it always refers to memory the same Table owns (either its own inline
// storage or its `slab` Box). The Table is heap-allocated and never
// moved post-adoption, so the pointer stays valid for the table's
// lifetime. No thread-unsafety concern: tables are accessed only
// through the Vm, single-threaded.
unsafe impl Send for Table {}
unsafe impl Sync for Table {}

impl Table {
    pub(crate) fn new(hdr: GcHeader) -> Table {
        Table {
            hdr,
            // P11-S5d.I — `array_ptr` is fixed up in
            // `Heap::new_table` after the Table reaches its final heap
            // address (so that `&inline_storage` is the heap address,
            // not a stack-local one). Null sentinel here so a
            // bug-detection invariant flags any pre-fixup read.
            array_ptr: std::ptr::null_mut(),
            slab: Box::new([]),
            asize: 0,
            inline_storage: [0; INLINE_U64S],
            nodes: Box::new([]),
            lastfree: 0,
            metatable: None,
            flags: 0,
        }
    }

    /// P11-S5d.I — set `array_ptr` to the inline storage's stable heap
    /// address. Called by `Heap::new_table` once the Table is at its
    /// final location.
    #[inline]
    pub(crate) fn init_array_ptr(&mut self) {
        self.array_ptr = self.inline_storage.as_mut_ptr() as *mut u8;
    }

    /// P11-S5d.H/I — read view onto the array-part tag bytes. Trails
    /// the avals portion in the active backing (inline or slab).
    #[inline(always)]
    pub(crate) fn atags(&self) -> &[u8] {
        let n = self.asize as usize;
        if n == 0 {
            return &[];
        }
        // SAFETY: `array_ptr` always points to a buffer with `n`
        // RawVal slots followed by `n` u8 tag bytes (either
        // `inline_storage` of `INLINE_U64S` u64s, or a `slab` of
        // `asize + ceil(asize/8)` u64s). The tag bytes start at byte
        // offset `n * 8` from the buffer base.
        unsafe {
            let ptr = self.array_ptr.add(n * 8);
            std::slice::from_raw_parts(ptr, n)
        }
    }

    #[inline(always)]
    pub(crate) fn atags_mut(&mut self) -> &mut [u8] {
        let n = self.asize as usize;
        if n == 0 {
            return &mut [];
        }
        // SAFETY: `array_ptr` was allocated by `Heap::init_array_ptr` with `array_cap` slots; the table holds it for its lifetime and the heap is single-threaded so no concurrent writers exist.
        unsafe {
            let ptr = self.array_ptr.add(n * 8);
            std::slice::from_raw_parts_mut(ptr, n)
        }
    }

    /// P11-S5d.H/I — read view onto the array-part payload slots. Sits
    /// at the start of the active backing (u64-aligned, identical size
    /// and layout to `RawVal`).
    #[inline(always)]
    pub(crate) fn avals(&self) -> &[RawVal] {
        let n = self.asize as usize;
        if n == 0 {
            return &[];
        }
        // SAFETY: inline_storage / slab both store u64s, so the cast
        // to `*const RawVal` is alignment-safe (RawVal size = 8,
        // align = 8). The buffer holds at least `n` such slots.
        unsafe { std::slice::from_raw_parts(self.array_ptr as *const RawVal, n) }
    }

    #[inline(always)]
    pub(crate) fn avals_mut(&mut self) -> &mut [RawVal] {
        let n = self.asize as usize;
        if n == 0 {
            return &mut [];
        }
        // SAFETY: `array_ptr` was allocated by `Heap::init_array_ptr` with `array_cap` slots; the table holds it for its lifetime and the heap is single-threaded so no concurrent writers exist.
        unsafe { std::slice::from_raw_parts_mut(self.array_ptr as *mut RawVal, n) }
    }

    /// Allocate a fresh external `[avals: asize × 8 bytes][atags: asize
    /// bytes]` slab. Only used when `asize > INLINE_ASIZE`. The buffer
    /// is u64-aligned via `Box<[u64]>` and zeroed (avals = `RawVal::
    /// NIL` aka `0`; atags = `raw::NIL` aka `0`).
    fn alloc_slab(asize: usize) -> Box<[u64]> {
        if asize == 0 {
            return Box::new([]);
        }
        let avals_u64s = asize;
        let atags_u64s = asize.div_ceil(8);
        let total = avals_u64s + atags_u64s;
        vec![0u64; total].into_boxed_slice()
    }

    /// This table's metatable, if any.
    pub fn metatable(&self) -> Option<Gc<Table>> {
        self.metatable
    }

    /// Install (or clear) this table's metatable. Does not perform any
    /// `__metatable` guarding; that belongs in the Vm-level `setmetatable`.
    pub fn set_metatable(&mut self, mt: Option<Gc<Table>>) {
        self.metatable = mt;
    }

    /// Bytes occupied by the table's *external* internal allocations
    /// (slab and nodes). Cheap O(1) read — Box len × element size, no
    /// allocator query. `Heap::free_obj` subtracts this on the way out
    /// so the credit applied via `set`/`rehash`/`ensure_*` is symmetric.
    ///
    /// P11-S5d.I — inline storage doesn't count toward this (it's part
    /// of the Table struct itself, accounted for by `size_of::<Table>()`
    /// at adoption time). When the array part lives inline, the slab
    /// is empty and contributes nothing here.
    pub(crate) fn internal_bytes(&self) -> usize {
        let n = self.asize as usize;
        let array_external = if n > INLINE_ASIZE as usize {
            n + n * std::mem::size_of::<RawVal>()
        } else {
            0
        };
        array_external + self.nodes.len() * std::mem::size_of::<Node>()
    }

    fn asize(&self) -> usize {
        self.asize as usize
    }

    fn aget(&self, idx: usize) -> Value {
        // SAFETY: callers gate on `idx < self.asize()` before reaching here
        // (`get_int`, `iter_array`, etc.). atags and avals are sized
        // identically by `rehash`, so a bound check passed against atags
        // covers avals too.
        unsafe {
            Value::pack(
                *self.atags().get_unchecked(idx),
                *self.avals().get_unchecked(idx),
            )
        }
    }

    fn aset(&mut self, idx: usize, v: Value) {
        let (t, b) = v.unpack();
        // SAFETY: see `aget`. callers (`set_norm`, `set_int`) gate on
        // `idx < self.asize()`. The two `*_mut` calls each take a
        // distinct `&mut self` borrow whose lifetime ends at the
        // statement boundary, so they don't overlap.
        unsafe {
            *self.atags_mut().get_unchecked_mut(idx) = t;
            *self.avals_mut().get_unchecked_mut(idx) = b;
        }
    }

    // ---- reads ----

    /// Raw lookup (no `__index` metamethod). Returns `Value::Nil` when
    /// the key is absent. `Value::Nil` and NaN floats return `nil` directly.
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

    /// Integer-keyed variant of [`Self::get`].
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
            // Dead-key slots carry a dangling Gc pointer whose memory may
            // have been reallocated to a different live object; raw_eq on
            // such a key can spuriously match the freshly-reused address.
            // Skip the comparison and only follow `next` (PUC `setdeadkey`
            // / `equalkey` short-circuit). 5.5 gc.lua :459-:478 was 12%
            // flaky on this exact path — a swept B-string's slot kept
            // chaining into A's slot, so `a[k] = nil` (k = A_string) hit
            // the dead slot and wrote nil there, leaving A's val untouched.
            if !n.dead_key && n.key.raw_eq(k) {
                return Some(idx);
            }
            if n.next == NONE {
                return None;
            }
            idx = n.next as usize;
        }
    }

    // ---- writes ----

    /// Insert / update `(key, val)`. `heap` is used to credit any internal
    /// Box growth (rehash) to `heap.bytes` so the counter stays in sync with
    /// real memory; `free_obj` subtracts `internal_bytes()` on the way out.
    pub fn set(&mut self, heap: &mut Heap, key: Value, val: Value) -> Result<(), TableError> {
        let k = normalize_set_key(key)?;
        self.set_norm(heap, k, val)
    }

    /// Integer-keyed variant of [`Self::set`].
    pub fn set_int(&mut self, heap: &mut Heap, i: i64, val: Value) -> Result<(), TableError> {
        self.set_norm(heap, Value::Int(i), val)
    }

    /// `k` is already normalized (no nil, no NaN, integral floats → Int).
    fn set_norm(&mut self, heap: &mut Heap, k: Value, v: Value) -> Result<(), TableError> {
        if let Value::Int(i) = k
            && i >= 1
            && (i as u64) <= self.asize() as u64
        {
            self.aset(i as usize - 1, v);
            return Ok(());
        }
        if let Some(idx) = self.find_node(k) {
            self.nodes[idx].val = v;
            return Ok(());
        }
        if v.is_nil() {
            return Ok(()); // absent key set to nil: nothing to record
        }
        self.insert_new(heap, k, v)
    }

    fn insert_new(&mut self, heap: &mut Heap, k: Value, v: Value) -> Result<(), TableError> {
        if self.nodes.is_empty() {
            self.rehash(heap, k)?;
            return self.set_norm(heap, k, v);
        }
        let mp = self.main_position(k);
        // A truly empty slot (key=Nil, !dead_key) is free for direct placement.
        // A dead-key slot still belongs to some chain (its `next` points to a
        // live entry the chain reaches), so we treat it as occupied here and
        // route the new key through the collision path below — that preserves
        // the back-links into this slot from other nodes' `next` fields.
        if self.nodes[mp].key.is_nil() && !self.nodes[mp].dead_key {
            self.nodes[mp] = Node {
                key: k,
                val: v,
                next: NONE,
                dead_key: false,
            };
            return Ok(());
        }
        let Some(free) = self.free_pos() else {
            self.rehash(heap, k)?;
            return self.set_norm(heap, k, v);
        };
        // Dead-key slot: it carries no live key, so by definition nobody else
        // counts it as "their main position owner". We give it directly to
        // the new key but preserve `next` so the chain it sits inside still
        // reaches its downstream entries.
        if self.nodes[mp].dead_key {
            let preserved_next = self.nodes[mp].next;
            self.nodes[mp] = Node {
                key: k,
                val: v,
                next: preserved_next,
                dead_key: false,
            };
            return Ok(());
        }
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
                dead_key: false,
            };
        } else {
            // colliding node owns this position: chain the new node behind it
            self.nodes[free] = Node {
                key: k,
                val: v,
                next: self.nodes[mp].next,
                dead_key: false,
            };
            self.nodes[mp].next = free as i32;
        }
        Ok(())
    }

    fn free_pos(&mut self) -> Option<usize> {
        while self.lastfree > 0 {
            self.lastfree -= 1;
            let n = &self.nodes[self.lastfree as usize];
            // Dead-key slots are still occupied for chain purposes (their
            // `next` may be the only path to a downstream entry) — don't
            // hand them out as free.
            if n.key.is_nil() && !n.dead_key {
                return Some(self.lastfree as usize);
            }
        }
        None
    }

    // ---- rehash (PUC luaH_rehash) ----

    fn rehash(&mut self, heap: &mut Heap, pending: Value) -> Result<(), TableError> {
        let mut nums = [0usize; 65];
        let mut int_keys = 0usize;
        let mut total = 1; // the pending key
        if let Value::Int(i) = pending
            && i >= 1
        {
            nums[ceil_log2(i as u64)] += 1;
            int_keys += 1;
        }
        let atags = self.atags();
        for (i, &tag) in atags.iter().enumerate() {
            if tag != raw::NIL {
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
        // PUC `luaH_resizearray` raises "table overflow" when the array part
        // would have to grow past MAXASIZE. luna mirrors with `MAX_ASIZE`,
        // checked on both the array and the hash bucket count (the latter is
        // a power-of-two of total - in_array entries).
        if new_asize > MAX_ASIZE {
            return Err(TableError::Overflow);
        }
        let hash_entries = total - in_array;
        if hash_entries > MAX_ASIZE {
            return Err(TableError::Overflow);
        }
        self.resize(heap, new_asize, hash_entries);
        Ok(())
    }

    /// Resize the table's array and hash parts. The array part grows
    /// (or shrinks) to `new_asize` NIL-initialized slots; the hash
    /// part rounds to the next power of two ≥ `hash_entries`. Any
    /// existing entries are re-inserted into the new layout. The
    /// Box growth is debited/credited to `heap.bytes` so `free_obj`
    /// can subtract the symmetric amount.
    ///
    /// P11-S5c.B — `Heap::new_table_sized` calls this on a freshly
    /// adopted empty table to pre-allocate the array part, sparing
    /// the table-fill loop from O(log N) intermediate `rehash`es.
    pub(crate) fn resize(&mut self, heap: &mut Heap, new_asize: usize, hash_entries: usize) {
        let before = self.internal_bytes();
        // P11-S5d.H/I — snapshot the old array entries before we
        // re-install the backing. The active buffer can be inline OR
        // slab; `array_ptr` already points to whichever it is, so
        // walking via raw offsets works the same for either case.
        let old_asize = self.asize as usize;
        let mut old_pairs: Vec<(u8, RawVal)> = Vec::with_capacity(old_asize);
        if old_asize > 0 {
            // SAFETY: `array_ptr` was set up by `Heap::new_table` or
            // an earlier `resize`; it covers `old_asize * 9` bytes
            // (avals + atags).
            let avals_base = self.array_ptr as *const RawVal;
            let atags_base = unsafe { self.array_ptr.add(old_asize * 8) as *const u8 };
            for i in 0..old_asize {
                // SAFETY: `i < array_len` is enforced by the surrounding loop bound; `atags_base` / `avals_base` point into the table's parallel arrays allocated in lockstep by `init_array_ptr`.
                let tag = unsafe { *atags_base.add(i) };
                // SAFETY: `i < array_len` is enforced by the surrounding loop bound; `atags_base` / `avals_base` point into the table's parallel arrays allocated in lockstep by `init_array_ptr`.
                let val = unsafe { *avals_base.add(i) };
                old_pairs.push((tag, val));
            }
        }
        let old_nodes = std::mem::take(&mut self.nodes);

        // Install the new array backing first, then update `array_ptr`
        // (before potentially dropping the old slab via the assignment
        // below) so the JIT never observes a stale pointer.
        self.asize = new_asize as u64;
        if new_asize <= INLINE_ASIZE as usize {
            // Inline path — zero the inline buffer; drop any prior
            // external slab.
            for slot in self.inline_storage.iter_mut() {
                *slot = 0;
            }
            self.array_ptr = self.inline_storage.as_mut_ptr() as *mut u8;
            self.slab = Box::new([]);
        } else {
            // External slab — allocate, then re-point `array_ptr`.
            self.slab = Self::alloc_slab(new_asize);
            self.array_ptr = self.slab.as_mut_ptr() as *mut u8;
        }

        let hsize = if hash_entries == 0 {
            0
        } else {
            hash_entries.next_power_of_two()
        };
        self.nodes = vec![Node::EMPTY; hsize].into_boxed_slice();
        self.lastfree = hsize as u32;
        // PUC `g->GCtotalbytes` analogue: credit (or debit) the box-size
        // delta so `Heap.bytes` reflects this table's actual internal
        // memory. `free_obj` subtracts `internal_bytes()` on the way out.
        let after = self.internal_bytes();
        heap.apply_bytes_delta(before, after);
        // Re-insert old array entries via the public set_norm path
        // (which handles rehashing if the new array shrinks below the
        // entry count).
        for (i, (tag, val)) in old_pairs.into_iter().enumerate() {
            if tag != raw::NIL {
                // SAFETY: `tag` and the raw value come from this table's parallel `atags` / `avals` arrays, which the table writers always keep in sync — the tag byte matches the raw payload's discriminator (see `runtime::value` `raw` module).
                let v = unsafe { Value::pack(tag, val) };
                let _ = self.set_norm(heap, Value::Int(i as i64 + 1), v);
            }
        }
        for n in old_nodes.iter() {
            if !n.val.is_nil() {
                let _ = self.set_norm(heap, n.key, n.val);
            }
        }
    }

    fn main_position(&self, k: Value) -> usize {
        debug_assert!(!self.nodes.is_empty());
        hash_key(k) as usize & (self.nodes.len() - 1)
    }

    // ---- length / iteration ----

    /// A border: `n` where `t[n]` is non-nil and `t[n+1]` is nil (PUC `luaH_getn`).
    /// This is Lua `#` semantics, not a container size — an `is_empty`
    /// counterpart would be meaningless.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> i64 {
        let asize = self.asize();
        let atags = self.atags();
        if asize > 0 && atags[asize - 1] == raw::NIL {
            // binary search inside the array part
            let (mut lo, mut hi) = (0usize, asize);
            while hi - lo > 1 {
                let m = lo + (hi - lo) / 2;
                if atags[m - 1] == raw::NIL {
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
                    // pathological sparse keys (the doubling overflowed): scan
                    // linearly from 1 for the first border, as PUC's
                    // unbound_search does — finds a small border fast instead of
                    // returning the huge one.
                    let mut i = 1i64;
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
        let atags = self.atags();
        for i in start..self.asize() {
            if atags[i] != raw::NIL {
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

    /// `(weak_keys, weak_values)` from the metatable's `__mode` field. Read by
    /// scanning the metatable for the `__mode` string (no interned key needed
    /// inside the collector).
    pub(crate) fn weak_mode(&self) -> (bool, bool) {
        let Some(mt) = self.metatable else {
            return (false, false);
        };
        for n in mt.nodes.iter() {
            if let (Value::Str(k), Value::Str(mode)) = (n.key, n.val)
                && k.as_bytes() == b"__mode"
            {
                let b = mode.as_bytes();
                return (b.contains(&b'k'), b.contains(&b'v'));
            }
        }
        (false, false)
    }

    /// True when this table holds at least one direct reference (array slot,
    /// hash key, or hash value) to a coroutine whose mark bit is still clear.
    /// Used by the GC's cycle-finalize check (PUC 5.3 gc.lua :502) to detect
    /// the table ↔ thread reference cycle that needs an extra GC round before
    /// `__gc` runs. Tag-level scan avoids walking the full reference graph.
    pub(crate) fn refs_contain_unmarked_coro(&self) -> bool {
        use crate::runtime::heap::header_is_marked;
        let atags = self.atags();
        let avals = self.avals();
        for (i, &tag) in atags.iter().enumerate() {
            if tag == raw::CORO {
                // SAFETY: raw union access — the tag byte at the same index in `atags` was previously confirmed to be `co` (closure/object pointer) so the `co` variant of `RawVal` holds the valid payload.
                let p = unsafe { avals[i].co } as *mut crate::runtime::heap::GcHeader;
                if !header_is_marked(p) {
                    return true;
                }
            }
        }
        for n in self.nodes.iter() {
            if let Value::Coro(co) = n.key {
                if !header_is_marked(co.as_ptr() as *mut crate::runtime::heap::GcHeader) {
                    return true;
                }
            }
            if let Value::Coro(co) = n.val {
                if !header_is_marked(co.as_ptr() as *mut crate::runtime::heap::GcHeader) {
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn trace(&self, m: &mut Marker) {
        let (wk, wv) = self.weak_mode();
        if wk || wv {
            m.weak.push(self as *const Table as *mut Table);
        }
        // weak keys + strong values = an ephemeron table: its hash values are
        // marked only if the key proves reachable (deferred to the convergence
        // pass), not here. PUC 5.1 predates ephemerons — under `no_ephemeron`
        // a weak-key table marks its values strongly during this pass, which
        // is what gc.lua's "weak tables" section requires.
        let ephemeron = wk && !wv && !m.no_ephemeron;
        if ephemeron {
            m.ephemeron.push(self as *const Table as *mut Table);
        }
        // array keys are integers (never weakly collected); skip values only
        // when the table has weak values
        if !wv {
            let atags = self.atags();
            let avals = self.avals();
            for (i, &tag) in atags.iter().enumerate() {
                if raw::is_gc(tag) {
                    // SAFETY: `tag` and the raw value come from this table's parallel `atags` / `avals` arrays, which the table writers always keep in sync — the tag byte matches the raw payload's discriminator (see `runtime::value` `raw` module).
                    m.value(unsafe { Value::pack(tag, avals[i]) });
                }
            }
        }
        for n in self.nodes.iter() {
            if !wk {
                m.value(n.key);
            }
            // ephemeron hash values are deferred; otherwise mark strong values
            if !wv && !ephemeron {
                m.value(n.val);
            }
        }
        if let Some(mt) = self.metatable {
            m.value(Value::Table(mt));
        }
    }

    /// Ephemeron pass: mark the value of every hash entry whose key is alive
    /// (`alive` decides — strong/marked keys, plus strings/numbers which are
    /// never weakly collected). Returns true if any value was newly marked, so
    /// the caller can iterate to a fixpoint (PUC `traverseephemeron`).
    pub(crate) fn converge_ephemeron(&self, alive: &dyn Fn(Value) -> bool, m: &mut Marker) -> bool {
        let mut changed = false;
        for n in self.nodes.iter() {
            if !n.val.is_nil() && alive(n.key) {
                changed |= m.value(n.val);
            }
        }
        changed
    }

    /// Clear entries whose weak key/value did not survive marking. `is_dead`
    /// reports whether a GC value was left unmarked (about to be swept).
    /// Clear weak-table entries whose key/value no longer carries a live
    /// reference. `is_dead` is a **pure** check (no side effects); the GC
    /// uses `mark_string` to resurrect any string that's still reachable via
    /// a *surviving* entry — Lua manual §2.5.4 says strings in weak tables
    /// are not collected as long as their entry is, and PUC `iscleared`
    /// implements that by marking the string during the same scan.
    pub(crate) fn clear_weak(
        &mut self,
        wk: bool,
        wv: bool,
        is_dead: &dyn Fn(Value) -> bool,
        mark_string: &dyn Fn(Value),
    ) {
        if wv {
            let n = self.asize as usize;
            for i in 0..n {
                let tag = self.atags()[i];
                if raw::is_gc(tag) {
                    // SAFETY: `tag` and the raw value come from this table's parallel `atags` / `avals` arrays, which the table writers always keep in sync — the tag byte matches the raw payload's discriminator (see `runtime::value` `raw` module).
                    let v = unsafe { Value::pack(tag, self.avals()[i]) };
                    if is_dead(v) {
                        self.atags_mut()[i] = raw::NIL;
                        self.avals_mut()[i] = RawVal::NIL;
                    } else {
                        mark_string(v);
                    }
                }
            }
        }
        for n in self.nodes.iter_mut() {
            if n.val.is_nil() {
                continue;
            }
            let key_dead = wk && is_dead(n.key);
            let val_dead = wv && is_dead(n.val);
            if key_dead || val_dead {
                // entry removed. PUC `setdeadkey`: when the key was a
                // collectable, drop the Gc pointer so a later raw_eq cannot
                // spuriously match a new object that gets allocated at the
                // same freed address. Keep `next` so the chain back-links
                // through this node still reach downstream entries; the
                // `dead_key` flag tells `find_node` to skip the comparison
                // and `insert_new` to treat the slot as a free
                // main-position owner that may inherit the chain.
                n.val = Value::Nil;
                if matches!(
                    n.key,
                    Value::Table(_)
                        | Value::Closure(_)
                        | Value::Native(_)
                        | Value::Coro(_)
                        | Value::Userdata(_)
                        | Value::Str(_)
                ) {
                    n.key = Value::Nil;
                    n.dead_key = true;
                }
            } else {
                // entry survives — resurrect any string reachable through it
                if wk {
                    mark_string(n.key);
                }
                if wv {
                    mark_string(n.val);
                }
            }
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
        Value::Closure(c) => mix64(c.as_ptr() as u64),
        Value::Native(n) => mix64(n.as_ptr() as u64),
        Value::Coro(co) => mix64(co.as_ptr() as u64),
        Value::Userdata(u) => mix64(u.as_ptr() as u64),
        Value::LightUserdata(p) => mix64(p as u64),
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

impl Table {
    /// Preallocate the array part (table.create); existing contents are
    /// preserved.
    pub fn ensure_array(&mut self, heap: &mut Heap, n: usize) {
        if n > self.asize() {
            let hash_entries = self.nodes.iter().filter(|nd| !nd.val.is_nil()).count();
            self.resize(heap, n, hash_entries);
        }
    }
}

impl Table {
    /// Preallocate hash-part capacity (table.create's second size).
    pub fn ensure_hash(&mut self, heap: &mut Heap, n: usize) {
        let entries = self.nodes.iter().filter(|nd| !nd.val.is_nil()).count();
        if n > self.nodes.len() {
            self.resize(heap, self.asize(), n.max(entries));
        }
    }
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
        with_table(|heap, t| {
            for i in 1..=1000 {
                let _ = t.set_int(heap, i, Value::Int(i * 10));
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
            t.set(heap, k1, Value::Int(1)).unwrap();
            t.set(heap, k2, Value::Int(2)).unwrap();
            t.set(heap, Value::Bool(true), Value::Int(3)).unwrap();
            t.set(heap, Value::Int(-5), Value::Int(4)).unwrap();
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
        with_table(|heap, t| {
            t.set(heap, Value::Float(2.0), Value::Int(22)).unwrap();
            assert!(t.get(Value::Int(2)).raw_eq(Value::Int(22)));
            t.set(heap, Value::Int(3), Value::Int(33)).unwrap();
            assert!(t.get(Value::Float(3.0)).raw_eq(Value::Int(33)));
            // -0.0 is key 0
            t.set(heap, Value::Float(-0.0), Value::Int(0)).unwrap();
            assert!(t.get(Value::Int(0)).raw_eq(Value::Int(0)));
            // non-integral floats are their own keys
            t.set(heap, Value::Float(0.5), Value::Int(55)).unwrap();
            assert!(t.get(Value::Float(0.5)).raw_eq(Value::Int(55)));
            assert!(t.get(Value::Int(0)).raw_eq(Value::Int(0)));
        });
    }

    #[test]
    fn bad_keys() {
        with_table(|heap, t| {
            assert_eq!(
                t.set(heap, Value::Nil, Value::Int(1)),
                Err(TableError::NilIndex)
            );
            assert_eq!(
                t.set(heap, Value::Float(f64::NAN), Value::Int(1)),
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
            t.set(heap, k, Value::Int(1)).unwrap();
            t.set(heap, k, Value::Nil).unwrap();
            assert!(t.get(k).is_nil());
            t.set(heap, k, Value::Int(2)).unwrap();
            assert!(t.get(k).raw_eq(Value::Int(2)));
            // setting an absent key to nil stays absent
            let k2 = Value::Str(heap.intern(b"k2"));
            t.set(heap, k2, Value::Nil).unwrap();
            assert!(t.get(k2).is_nil());
        });
    }

    #[test]
    fn borders_with_holes() {
        with_table(|heap, t| {
            let _ = t.set_int(heap, 1, Value::Int(1));
            let _ = t.set_int(heap, 2, Value::Int(2));
            assert_eq!(t.len(), 2);
            t.set_int(heap, 2, Value::Nil).unwrap();
            assert_is_border(t, t.len());
            // hash-resident tail
            let _ = t.set_int(heap, 1_000_000, Value::Int(1));
            assert_is_border(t, t.len());
        });
    }

    #[test]
    fn len_on_empty_and_hash_only() {
        with_table(|heap, t| {
            assert_eq!(t.len(), 0);
            let xk = Value::Str(heap.intern(b"x"));
            t.set(heap, xk, Value::Int(1)).unwrap();
            assert_eq!(t.len(), 0);
        });
    }

    #[test]
    fn next_iterates_everything_exactly_once() {
        with_table(|heap, t| {
            let mut expected = 0i64;
            for i in 1..=64 {
                let _ = t.set_int(heap, i, Value::Int(i));
                expected += i;
            }
            for i in 0..32 {
                let k = Value::Str(heap.intern(format!("s{i}").as_bytes()));
                t.set(heap, k, Value::Int(1000 + i)).unwrap();
                expected += 1000 + i;
            }
            t.set(heap, Value::Float(2.5), Value::Int(7)).unwrap();
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
            let _ = t.set_int(heap, 1, Value::Int(1));
            let _ = t.set_int(heap, 3, Value::Int(3));
            let k = Value::Str(heap.intern(b"gone"));
            t.set(heap, k, Value::Int(9)).unwrap();
            t.set(heap, k, Value::Nil).unwrap();
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
        with_table(|heap, t| {
            // dense negative ints all land in the hash part; with identity
            // hashing they exercise both chain cases heavily
            for i in 0..512 {
                let _ = t.set_int(heap, -i, Value::Int(i));
            }
            for i in 0..512 {
                assert!(t.get_int(-i).raw_eq(Value::Int(i)), "lost key {}", -i);
            }
        });
    }

    #[test]
    fn rehash_redistributes_into_array() {
        with_table(|heap, t| {
            // insert 1..n in reverse: starts in hash, rehash must migrate
            for i in (1..=256).rev() {
                let _ = t.set_int(heap, i, Value::Int(i));
            }
            assert_eq!(t.len(), 256);
            for i in 1..=256 {
                assert!(t.get_int(i).raw_eq(Value::Int(i)));
            }
        });
    }
}
