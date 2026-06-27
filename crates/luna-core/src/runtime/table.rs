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

/// v2.1 Phase 1I.B — JIT layout constants for table-field IC.
///
/// luna-jit's trace lowerer needs to emit direct loads against
/// `Table.nodes` (the hash part) without paying the helper-call ABI
/// for each `Op::GetField` / `Op::SetField`. These constants expose
/// the field offsets so the cranelift IR can be parameterised at
/// compile time. The `Node` struct itself remains `pub(crate)` — only
/// the offsets cross the crate boundary.
///
/// Layout assumptions:
/// - `Box<[Node]>` is a fat pointer `(data_ptr, len)` on 64-bit
///   targets (16 bytes total). The data pointer occupies the low 8
///   bytes, length the high 8. This is the de-facto Rust ABI for
///   `Box<[T]>` / `&[T]` but isn't formally guaranteed; the unit
///   test `phase_1i_b_node_layout_pinned` and the const assertion
///   on `size_of::<Box<[Node]>>()` catch drift.
/// - `Value` is `#[repr(C, u8)]` so the discriminant byte sits at
///   offset 0 and the payload starts at offset 8 (after 7 bytes of
///   alignment padding). Total size 16 bytes per the existing
///   `value_is_16_bytes` test in `runtime/value.rs`.
/// - `Node` is `#[derive(Clone, Copy)]` with field order
///   `(key: Value, val: Value, next: i32, dead_key: bool)`, so
///   `key` lives at offset 0 and `val` at offset 16. The trailing
///   `next + dead_key` fields are not read by the IC.
pub mod jit_layout {
    use super::Node;
    use crate::runtime::Table;

    /// Byte offset of the `nodes: Box<[Node]>` field within `Table`.
    /// The fat-ptr low word (data ptr) lives at this offset; the
    /// high word (length) at `TABLE_NODES_OFFSET + 8`. luna-jit
    /// adds `TABLE_NODES_PTR_OFFSET` / `TABLE_NODES_LEN_OFFSET`
    /// constants in `jit_backend/mod.rs` to express that split.
    pub const TABLE_NODES_OFFSET: usize = std::mem::offset_of!(Table, nodes);

    /// Byte offset of `key: Value` within `Node` (= 0).
    pub const NODE_KEY_OFFSET: usize = std::mem::offset_of!(Node, key);

    /// Byte offset of `val: Value` within `Node` (= 16 — `key` is 16-byte
    /// `Value`, no inner padding).
    pub const NODE_VAL_OFFSET: usize = std::mem::offset_of!(Node, val);

    /// Total `Node` size in bytes (= 40 on 64-bit). Used as the stride
    /// in `node_addr = nodes_ptr + slot_idx * SIZEOF_NODE`.
    pub const SIZEOF_NODE: usize = std::mem::size_of::<Node>();

    /// Static guard: pin the assumptions luna-jit relies on at compile
    /// time. Layout drift here breaks IR emit, so trap it at compile
    /// time rather than at trace-fire time.
    const _: () = {
        assert!(std::mem::size_of::<Box<[Node]>>() == 16);
        assert!(NODE_KEY_OFFSET == 0);
        assert!(NODE_VAL_OFFSET == 16);
        assert!(SIZEOF_NODE >= 32);
    };
}

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

/// C3 — SoA Robin Hood meta-word layout (Variant A, see
/// `.dev/rfcs/v2.0-c3-soa-robinhood-rfc.md` §5.1).
///
/// Each `meta[idx]` slot encodes the open-addressing slot state in a
/// single u16:
/// - bit 15 (`OCCUPIED_BIT`): 0 = empty, 1 = occupied
/// - bit 14 (`TOMBSTONE_BIT`): 0 = live, 1 = tombstoned-occupied
/// - bits 13..0 (`PSL_MASK`): probe-sequence length (0..16383)
///
/// The 14-bit PSL field is **far** beyond any realistic Robin Hood
/// max-PSL at load ≤ 0.75 (expected max ~20 on 1024 slots; even the
/// long-tail outliers seen empirically with luna's existing hash
/// distributions stay under 200). 2 bytes/slot is still 20× smaller
/// than the 40-byte Node — the SoA bandwidth gain (§3.5 of the RFC)
/// is preserved.
///
/// Earlier draft used 1 byte with a 6-bit PSL cap of 63; bench under
/// load 0.676 on cap=1024 produced a long-tail PSL of 64+ with the
/// LuaStr+mix64 hash distribution, forcing the widening.
///
/// Tombstones do NOT free the slot for `find` (probe continues past), but
/// DO free it for `insert` (write the new entry, clear the tomb bit). They
/// accumulate; the rehash path compacts them periodically.
#[allow(dead_code)]
pub(crate) mod meta_bits {
    pub const OCCUPIED_BIT: u16 = 0b1000_0000_0000_0000;
    pub const TOMBSTONE_BIT: u16 = 0b0100_0000_0000_0000;
    pub const PSL_MASK: u16 = 0b0011_1111_1111_1111;
    pub const PSL_MAX: u16 = PSL_MASK;
    /// Empty slot — bit 15 = 0, all others 0.
    pub const EMPTY: u16 = 0;

    #[inline(always)]
    pub fn is_occupied(m: u16) -> bool {
        (m & OCCUPIED_BIT) != 0
    }
    #[inline(always)]
    pub fn is_tombstone(m: u16) -> bool {
        (m & TOMBSTONE_BIT) != 0
    }
    /// Live = occupied AND not tombstoned. `next()` iteration cursor returns
    /// these. `find_slot_rh` short-circuits on a live match.
    #[inline(always)]
    pub fn is_live(m: u16) -> bool {
        (m & (OCCUPIED_BIT | TOMBSTONE_BIT)) == OCCUPIED_BIT
    }
    #[inline(always)]
    pub fn psl(m: u16) -> u16 {
        m & PSL_MASK
    }
    #[inline(always)]
    pub fn pack(psl: u16, tomb: bool) -> u16 {
        debug_assert!(psl <= PSL_MAX);
        let mut m = OCCUPIED_BIT | (psl & PSL_MASK);
        if tomb {
            m |= TOMBSTONE_BIT;
        }
        m
    }
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
    /// C3 — SoA Robin Hood hash part (Variant A, parallel to `nodes`
    /// during Phase B+C transition). After Phase 4 cutover these
    /// replace `nodes` entirely. Each of `keys` / `vals` / `meta`
    /// is the same length as `nodes` and is sized in lockstep by
    /// `resize`. `meta` byte layout per the `meta_bits` module above.
    /// Empty `Box::new([])` until Phase D cuts over.
    pub(crate) keys: Box<[Value]>,
    pub(crate) vals: Box<[Value]>,
    pub(crate) meta: Box<[u16]>,
    /// Count of tombstoned-occupied meta slots; rehash trigger.
    pub(crate) tombstones: u32,
    /// C3 — iterator-guard counter (R-A3 mitigation). Incremented on
    /// each `pairs`/`next` entry, decremented on exit. While > 0,
    /// the SoA path MUST defer rehash (which would rebase slot
    /// indices and break PUC `nextvar.lua:520-521` invariant).
    /// Phase F wires this; Phase B initialises to 0.
    pub(crate) iter_depth: u32,
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
            keys: Box::new([]),
            vals: Box::new([]),
            meta: Box::new([]),
            tombstones: 0,
            iter_depth: 0,
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
        let soa_external = self.keys.len() * std::mem::size_of::<Value>()
            + self.vals.len() * std::mem::size_of::<Value>()
            + self.meta.len() * std::mem::size_of::<u16>();
        array_external + self.nodes.len() * std::mem::size_of::<Node>() + soa_external
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

    /// String-keyed variant of [`Self::get`] for v1.2 D4 A1 GetField fast
    /// path: the GetField interp arm always has a `Gc<LuaStr>` key from
    /// `Proto.consts`. Skips the outer `Value` match (which would only
    /// take the `_ => self.get_hash(k)` arm anyway) so the dispatcher
    /// pays one less branch per call. ~5 GetField/iter × 1000 iters/cell
    /// on the Redis-Lua-shape workload — every shaved nanosecond shows
    /// up at the bench level. Counter-validated via
    /// `examples/diag_opcode_breakdown.rs`.
    #[inline]
    pub fn get_str(&self, key: crate::runtime::Gc<crate::runtime::string::LuaStr>) -> Value {
        self.get_hash(Value::Str(key))
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

    /// PUC `luaV_fastset` / `luaV_finishfastset` analogue: single-walk
    /// in-place update for an existing key. Returns `true` iff `key` is
    /// present with a non-nil value and the slot was overwritten with
    /// `val`. Returns `false` when the key is absent, the slot holds nil,
    /// or the key normalisation rejects it — the caller is then expected
    /// to run the `__newindex` chain or fall back to `set` for the raw
    /// insert.
    ///
    /// Collapses the SetField hot path from two hash-chain walks
    /// (`get` + `set`) to one. The `__newindex` invariant ("fires iff
    /// `get` would have returned nil") is preserved because this method
    /// writes only when the existing slot is non-nil — the exact set the
    /// prior `tb.get(key).is_nil()` gate already excluded from
    /// `__newindex` eligibility. See
    /// `.dev/rfcs/v2.0-pi-phase2-a3-audit.md` §4 for the case-by-case
    /// semantics check.
    ///
    /// The caller is responsible for firing `Heap::barrier_back` after a
    /// `true` return (same contract as the surrounding `raw_set`
    /// wrapper).
    pub fn try_set_existing(&mut self, key: Value, val: Value) -> bool {
        let k = match normalize_set_key(key) {
            Ok(k) => k,
            Err(_) => return false,
        };
        if let Value::Int(i) = k
            && i >= 1
            && (i as u64) <= self.asize() as u64
        {
            let idx = i as usize - 1;
            // SAFETY: `idx < self.asize()` is guarded by the conditional
            // above, mirroring the bound on `aget`/`aset`.
            let tag = unsafe { *self.atags().get_unchecked(idx) };
            if tag != raw::NIL {
                // Nil-val on a live slot must follow the same tombstone
                // discipline as `set_norm` — routed through
                // `clear_existing_slot` so chain-world / future
                // data-layout cutovers (Phase E SoA) stay aligned.
                // See `.dev/known-bugs/fixed/`.
                if val.is_nil() {
                    self.clear_existing_slot(k);
                } else {
                    self.aset(idx, val);
                }
                return true;
            }
            // Array slot present-but-nil → __newindex eligible: do NOT
            // write. Caller falls through to the metamethod chain.
            return false;
        }
        if let Some(idx) = self.find_node(k)
            && !self.nodes[idx].val.is_nil()
        {
            if val.is_nil() {
                self.clear_existing_slot(k);
            } else {
                self.nodes[idx].val = val;
            }
            return true;
        }
        false
    }

    /// Shared "live with val=Nil is illegal" tombstone routine for the
    /// two write entry points (`set_norm` and `try_set_existing`). The
    /// slot must already be known live (array slot inside `asize()` /
    /// node returned by `find_node`).
    ///
    /// Chain-world today:
    ///   - array slot → `aset(_, Nil)` clears the atag, so `next()`'s
    ///     `tag != raw::NIL` filter skips the slot.
    ///   - node slot  → soft tombstone (key kept, `val = Nil`); chain
    ///     `next()` filter `!n.val.is_nil()` skips it, and `find_node`
    ///     still routes a future re-insert into the same slot without
    ///     a rehash.
    ///
    /// Centralising the discipline here lets a future Phase E SoA
    /// cutover (Variant B linear probe, or any non-Robin-Hood layout
    /// attack that switches `next()`'s filter to `meta_bits::is_live`)
    /// migrate both entry points in lockstep — exactly the divergence
    /// that surfaced as `(key, nil)` zombies in `pairs()` on the C3
    /// Session 2 cutover branch.
    fn clear_existing_slot(&mut self, k: Value) {
        if let Value::Int(i) = k
            && i >= 1
            && (i as u64) <= self.asize() as u64
        {
            self.aset(i as usize - 1, Value::Nil);
            return;
        }
        if let Some(idx) = self.find_node(k) {
            self.nodes[idx].val = Value::Nil;
        }
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
            // Live array slot + Nil write goes through the shared
            // tombstone routine (see `clear_existing_slot` for the
            // chain ↔ future-SoA rationale). The non-Nil branch is
            // identical to a bare `aset` today.
            if v.is_nil() {
                self.clear_existing_slot(k);
            } else {
                self.aset(i as usize - 1, v);
            }
            return Ok(());
        }
        if let Some(idx) = self.find_node(k) {
            if v.is_nil() {
                self.clear_existing_slot(k);
            } else {
                self.nodes[idx].val = v;
            }
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

// =====================================================================
// C3 — SoA + Robin Hood open-addressing hash part (Variant A).
//
// Parallel to the chain-walk path during Phase B+C+D transition: the
// chain `nodes` / `lastfree` is the authoritative read path until
// Phase E migrates `next()` and Phase 4 cuts over. These methods
// operate only on the `keys` / `vals` / `meta` / `tombstones` SoA
// arrays — chain state is never touched.
//
// Layout invariants the methods below maintain:
//   - `keys.len() == vals.len() == meta.len()`, all power-of-two
//     (or zero in the empty-stub state)
//   - `meta[i] = meta_bits::EMPTY` iff slot i is free
//   - tombstoned slots are scanned past by find but reused by insert
//   - `tombstones` counts the meta slots with TOMBSTONE_BIT set
//   - load factor (live + tombstone) / cap is kept ≤ 0.75 via
//     `soa_grow_if_needed` (R-A1 mitigation — PSL bound 63)
//   - rehash is REFUSED when `iter_depth > 0` (R-A3 mitigation —
//     wired in Phase F; for Phase C the counter is always 0 so the
//     refusal path is unreachable)
//
// Refs: `.dev/rfcs/v2.0-c3-soa-robinhood-rfc.md` §5.1 (variant A),
// §6.2 Phase 1-3 (impl plan).
// =====================================================================

/// C3 — initial SoA capacity when growing from empty. Power of two.
/// Picked at 4 so a 3-element table doesn't trigger an immediate
/// regrowth.
#[allow(dead_code)] // wired by Phase D/E/F integration
pub(crate) const SOA_INITIAL_CAP: usize = 4;

/// C3 — high load-factor threshold (3/4). SoA grow trigger; matches
/// the RFC §5.1 recommendation. PSL_MAX is the u16 14-bit value so
/// long-tail PSL overruns are recoverable via grow-retry.
#[allow(dead_code)]
const SOA_LOAD_NUM: usize = 3;
#[allow(dead_code)]
const SOA_LOAD_DEN: usize = 4;

/// C3 — tombstone density threshold (1/4). When tombstones/cap ≥ 25%
/// the next non-resize-triggering rehash compacts them.
#[allow(dead_code)]
const SOA_TOMB_NUM: usize = 1;
#[allow(dead_code)]
const SOA_TOMB_DEN: usize = 4;

#[allow(dead_code)] // wired by Phase D/E/F integration into public set/get/next
impl Table {
    /// C3 — current SoA hash-part capacity in slots (0 = empty stub).
    #[inline]
    pub(crate) fn soa_cap(&self) -> usize {
        self.meta.len()
    }

    /// C3 — count of live (occupied & not tombstone) SoA slots.
    /// O(n) — only used by the Phase G equivalence test path; the
    /// hot rehash trigger uses `live_estimate = cap*3/4 - tombstones`
    /// implicitly via `soa_grow_if_needed`.
    #[cfg(test)]
    pub(crate) fn soa_live_count(&self) -> usize {
        self.meta.iter().filter(|&&m| meta_bits::is_live(m)).count()
    }

    /// C3 — count of occupied (live OR tombstoned) SoA slots; this is
    /// the value the load factor compares against `cap * 3/4`.
    #[inline]
    fn soa_occupied_count(&self) -> usize {
        // O(n) sweep — Phase C inserts the count on each call so the
        // worst case is bounded by per-insert amortised cost. A future
        // polish could maintain a counter incrementally; left as a
        // Phase H mini-bench-driven follow-up if PI sample shows it
        // contributes > 1 µs/cell.
        self.meta
            .iter()
            .filter(|&&m| meta_bits::is_occupied(m))
            .count()
    }

    /// C3 — Robin Hood lookup. Returns the slot index of a *live*
    /// matching key, or None if absent. Walks past tombstones (they
    /// preserve probe chains). Returns None if the SoA cap is zero
    /// (Phase B stub state). Bound by `cap` probes; in practice
    /// expected ≤ 8 at load 0.75.
    pub(crate) fn soa_find_slot(&self, k: Value) -> Option<usize> {
        let cap = self.meta.len();
        if cap == 0 {
            return None;
        }
        let mask = cap - 1;
        let mut idx = (hash_key(k) as usize) & mask;
        // Walk until empty slot or wrap. The `steps <= cap` bound
        // is a safety net: a properly maintained Robin Hood table
        // with load < 1 always has at least one empty slot, so a
        // full wrap means table invariant violation.
        for _ in 0..cap {
            let m = self.meta[idx];
            if !meta_bits::is_occupied(m) {
                return None;
            }
            if !meta_bits::is_tombstone(m) && self.keys[idx].raw_eq(k) {
                return Some(idx);
            }
            idx = (idx + 1) & mask;
        }
        None
    }

    /// C3 — Allocate fresh SoA arrays at `new_cap` (power of two) and
    /// re-insert every live entry from the old SoA arrays. Tombstones
    /// are dropped (count resets to 0). Used by `soa_grow_if_needed`
    /// (new_cap = max(SOA_INITIAL_CAP, 2*cap)) and by Phase D
    /// tombstone compaction (new_cap = cap).
    ///
    /// IMPORTANT: rehash MUST NOT fire while `iter_depth > 0`
    /// (R-A3) — wired in Phase F. Phase C callers all enter from
    /// non-iteration paths.
    fn soa_rehash_to(&mut self, heap: &mut Heap, new_cap: usize) -> Result<(), TableError> {
        debug_assert!(new_cap.is_power_of_two() && new_cap > 0);
        let before = self.internal_bytes();
        // Snapshot old live entries. This list is the canonical
        // "must be present after rehash" set; we restart from it on
        // any PSL-overflow retry.
        let mut survivors: Vec<(Value, Value)> = Vec::with_capacity(self.meta.len());
        for i in 0..self.meta.len() {
            if meta_bits::is_live(self.meta[i]) {
                survivors.push((self.keys[i], self.vals[i]));
            }
        }
        // Install fresh empty arrays at `new_cap`. On PSL overflow
        // during the re-insert pass (extremely rare with the 14-bit
        // PSL budget — would need a pathological hash distribution),
        // double the cap and replay the original `survivors` list
        // from scratch. We don't try to salvage partial work — the
        // rare-path retry cost is bounded by O(n × max_doublings),
        // and max_doublings has a hard MAX_ASIZE ceiling.
        let mut cap = new_cap;
        loop {
            if cap > MAX_ASIZE {
                return Err(TableError::Overflow);
            }
            self.keys = vec![Value::Nil; cap].into_boxed_slice();
            self.vals = vec![Value::Nil; cap].into_boxed_slice();
            self.meta = vec![meta_bits::EMPTY; cap].into_boxed_slice();
            self.tombstones = 0;
            let mut overflowed = false;
            for (k, v) in survivors.iter().copied() {
                if self.soa_place_known_absent(k, v).is_err() {
                    overflowed = true;
                    break;
                }
            }
            if !overflowed {
                break;
            }
            cap = cap.checked_mul(2).ok_or(TableError::Overflow)?;
        }
        let after = self.internal_bytes();
        heap.apply_bytes_delta(before, after);
        Ok(())
    }

    /// C3 — Raw rob-from-rich placement for a key known to be absent
    /// from the SoA arrays. Used by `soa_rehash_to` (re-insert pass)
    /// and by `soa_insert` (new-key path after the explicit
    /// soa_find_slot check). This routine does NOT auto-grow on a
    /// load-factor trigger (caller's responsibility), but DOES signal
    /// back to the caller via `Err(())` when the PSL bound of 63 is
    /// hit before finding an empty slot — Robin Hood's long-tail
    /// max-PSL exceeds the 6-bit storage budget at unfavourable hash
    /// distributions even under the nominal 0.75 load gate (RFC §6.3
    /// R-A1). The caller (`soa_insert`) handles by growing & retrying.
    ///
    /// On success returns the slot index where the new key landed
    /// (after any rob-from-rich shuffle, the original `k` value is at
    /// this returned index).
    fn soa_place_known_absent(&mut self, k: Value, v: Value) -> Result<usize, (Value, Value)> {
        let cap = self.meta.len();
        debug_assert!(cap > 0);
        let mask = cap - 1;
        let landing = (hash_key(k) as usize) & mask;
        let mut idx = landing;
        let mut cur_psl: u16 = 0;
        let mut cur_key = k;
        let mut cur_val = v;
        let mut placed_at: Option<usize> = None;
        for _ in 0..cap {
            let m = self.meta[idx];
            if !meta_bits::is_occupied(m) || meta_bits::is_tombstone(m) {
                if meta_bits::is_tombstone(m) {
                    self.tombstones = self.tombstones.saturating_sub(1);
                }
                self.meta[idx] = meta_bits::pack(cur_psl, false);
                self.keys[idx] = cur_key;
                self.vals[idx] = cur_val;
                return Ok(placed_at.unwrap_or(idx));
            }
            let stored_psl = meta_bits::psl(m);
            if cur_psl > stored_psl {
                // Rob: swap cur into this slot, evict stored to continue.
                std::mem::swap(&mut cur_key, &mut self.keys[idx]);
                std::mem::swap(&mut cur_val, &mut self.vals[idx]);
                self.meta[idx] = meta_bits::pack(cur_psl, false);
                if placed_at.is_none() {
                    placed_at = Some(idx);
                }
                cur_psl = stored_psl;
            }
            idx = (idx + 1) & mask;
            cur_psl = cur_psl.saturating_add(1);
            if cur_psl > meta_bits::PSL_MAX {
                // PSL exceeds the 14-bit storage budget — exceptionally
                // rare with 16384 max. Caller (soa_insert / rehash
                // outer loop) handles by growing & retrying. Partial
                // state: all entries are still in the table EXCEPT
                // `(cur_key, cur_val)` which is the latest homeless
                // evictee — return it so caller can re-issue.
                return Err((cur_key, cur_val));
            }
        }
        // Wrapped cap probes with no free slot — invariant violation
        // (load < 1 should guarantee at least one empty). Signal as
        // PSL-overflow equivalent so caller grows + retries.
        Err((cur_key, cur_val))
    }

    /// C3 — Grow SoA capacity if the load factor is at or above the
    /// 0.75 trigger. Doubles cap; from empty grows to SOA_INITIAL_CAP.
    fn soa_grow_if_needed(&mut self, heap: &mut Heap) -> Result<(), TableError> {
        // Defer rehash when an iterator is in flight (R-A3). Wired
        // in Phase F; in Phase C iter_depth is always 0.
        if self.iter_depth > 0 {
            return Ok(());
        }
        let cap = self.meta.len();
        if cap == 0 {
            return self.soa_rehash_to(heap, SOA_INITIAL_CAP);
        }
        let occupied = self.soa_occupied_count();
        if occupied * SOA_LOAD_DEN >= cap * SOA_LOAD_NUM {
            let new_cap = cap.checked_mul(2).ok_or(TableError::Overflow)?;
            return self.soa_rehash_to(heap, new_cap);
        }
        // Tombstone compaction (same cap, drops tombstones).
        if self.tombstones as usize * SOA_TOMB_DEN >= cap * SOA_TOMB_NUM {
            return self.soa_rehash_to(heap, cap);
        }
        Ok(())
    }

    /// C3 — Insert (or update) `(k, v)` in the SoA hash part. Routes
    /// through `soa_find_slot` first so an existing key updates its
    /// val in place; otherwise rob-from-rich places a new entry.
    /// Auto-rehashes if the load factor would exceed 0.75 OR if the
    /// place chain runs into a PSL overflow on a pathological hash
    /// distribution.
    ///
    /// Phase C: this method is callable from outside via the
    /// equivalence-test entrypoint (Phase G); not yet hooked into
    /// public `set` / `set_norm`.
    pub(crate) fn soa_insert(
        &mut self,
        heap: &mut Heap,
        k: Value,
        v: Value,
    ) -> Result<(), TableError> {
        debug_assert!(!matches!(k, Value::Nil));
        // 1. Update-in-place if key is already present (live slot).
        if let Some(idx) = self.soa_find_slot(k) {
            self.vals[idx] = v;
            return Ok(());
        }
        // 2. New key: ensure capacity, then place. On PSL-overflow
        // from the place chain (extremely rare with 14-bit PSL budget),
        // grow + rehash with the homeless evictee merged in.
        // `soa_rehash_with_extra` handles further retries internally,
        // bounded by MAX_ASIZE.
        self.soa_grow_if_needed(heap)?;
        match self.soa_place_known_absent(k, v) {
            Ok(_) => Ok(()),
            Err(homeless) => {
                let cap = self.meta.len();
                let new_cap = cap.checked_mul(2).ok_or(TableError::Overflow)?;
                self.soa_rehash_with_extra(heap, new_cap, homeless)
            }
        }
    }

    /// C3 — Rehash to `new_cap` while merging in an extra (k, v) pair
    /// not currently in the SoA arrays. Used by `soa_insert` to
    /// recover from PSL overflow: the homeless evictee from the failed
    /// place chain gets appended to the survivor list before the
    /// re-insert pass.
    fn soa_rehash_with_extra(
        &mut self,
        heap: &mut Heap,
        new_cap: usize,
        extra: (Value, Value),
    ) -> Result<(), TableError> {
        let before = self.internal_bytes();
        let mut survivors: Vec<(Value, Value)> = Vec::with_capacity(self.meta.len() + 1);
        for i in 0..self.meta.len() {
            if meta_bits::is_live(self.meta[i]) {
                survivors.push((self.keys[i], self.vals[i]));
            }
        }
        // Avoid duplicating the extra if its key was already placed at
        // some slot during the failed rob chain (the rob may have
        // landed the original input into a slot before overflowing on
        // a downstream evictee — that case the meta-walk above picks
        // it up).
        if !survivors.iter().any(|(k, _)| k.raw_eq(extra.0)) {
            survivors.push(extra);
        }
        let mut cap = new_cap;
        loop {
            if cap > MAX_ASIZE {
                return Err(TableError::Overflow);
            }
            self.keys = vec![Value::Nil; cap].into_boxed_slice();
            self.vals = vec![Value::Nil; cap].into_boxed_slice();
            self.meta = vec![meta_bits::EMPTY; cap].into_boxed_slice();
            self.tombstones = 0;
            let mut overflowed = false;
            for (k, v) in survivors.iter().copied() {
                if self.soa_place_known_absent(k, v).is_err() {
                    overflowed = true;
                    break;
                }
            }
            if !overflowed {
                break;
            }
            cap = cap.checked_mul(2).ok_or(TableError::Overflow)?;
        }
        let after = self.internal_bytes();
        heap.apply_bytes_delta(before, after);
        Ok(())
    }

    /// C3 — Read SoA hash part. Mirrors `get_hash` but reads from
    /// keys/vals/meta rather than nodes. Used by the Phase G
    /// equivalence test; not yet hooked into public `get` / `get_hash`.
    pub(crate) fn soa_get(&self, k: Value) -> Value {
        match self.soa_find_slot(k) {
            Some(idx) => self.vals[idx],
            None => Value::Nil,
        }
    }

    /// C3 — Tombstone deletion. Marks the live slot for `k` as
    /// tombstoned, preserving the slot index (no backward shift).
    /// Slot-index stability is the PUC `next()` iteration invariant
    /// — `nextvar.lua:520-521` requires that deleting prior keys
    /// during a `pairs` traversal does NOT move unvisited keys.
    /// Backward-shift deletion would violate this; tombstones are
    /// the standard Robin Hood resolution (see RFC §4.5 + §5.1).
    ///
    /// keys[idx] / vals[idx] are reset to Nil so the GC marker is
    /// not held to the previous entries — only the tombstone bit
    /// distinguishes "occupied tombstone" from "free empty".
    ///
    /// Returns true if the key was found and deleted, false if absent.
    ///
    /// Phase D: not yet hooked into public `set(k, Nil)` — wired in
    /// Phase E alongside the `next()` migration.
    pub(crate) fn soa_delete(&mut self, k: Value) -> bool {
        if let Some(idx) = self.soa_find_slot(k) {
            let psl = meta_bits::psl(self.meta[idx]);
            self.meta[idx] = meta_bits::pack(psl, true);
            self.keys[idx] = Value::Nil;
            self.vals[idx] = Value::Nil;
            self.tombstones = self.tombstones.saturating_add(1);
            true
        } else {
            false
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

    /// v2.1 Phase 1I.B — pin `Box<[Node]>` fat-ptr layout at runtime.
    /// The luna-jit table-field IC reads `(ptr, len)` directly out of
    /// the `nodes` field assuming the data pointer occupies the low 8
    /// bytes and the length the high 8 bytes (de-facto Rust ABI on
    /// 64-bit targets but not formally guaranteed). If a future Rust
    /// release reorders the fat-ptr, this test fails before IC fires
    /// at runtime.
    #[test]
    fn phase_1i_b_node_layout_pinned() {
        use jit_layout::*;
        assert_eq!(std::mem::size_of::<Box<[Node]>>(), 16);
        assert_eq!(NODE_KEY_OFFSET, 0);
        assert_eq!(NODE_VAL_OFFSET, 16);
        assert!(SIZEOF_NODE >= 32);

        // Construct a real Box<[Node]> with a known length, then
        // peek at the fat-pointer's two halves to confirm the
        // (data_ptr, len) order. Use a 4-slot box so the length is
        // non-zero and the data pointer is heap-allocated.
        let b: Box<[Node]> = vec![Node::EMPTY; 4].into_boxed_slice();
        let raw_ptr = b.as_ptr();
        let raw_len = b.len();
        // SAFETY: reading the fat pointer's two words is exactly the
        // layout luna-jit's IR assumes; it's the safest possible test
        // of that assumption.
        let words: [usize; 2] = unsafe { std::mem::transmute_copy(&b) };
        assert_eq!(words[0], raw_ptr as usize, "fat-ptr low word = data ptr");
        assert_eq!(words[1], raw_len, "fat-ptr high word = len");
        drop(b);
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

    // -----------------------------------------------------------------
    // C3 SoA Robin Hood equivalence tests (Phase G).
    //
    // Cross-check the new SoA + RH path against the existing chain-walk
    // path: replay the same insert/lookup sequence on a table via
    // `set` (chain) and another via `soa_insert` (SoA), then assert
    // `get == soa_get` for every key.
    //
    // Phase B+C scope: insert + read only. Tombstone delete equivalence
    // arrives with Phase D.
    // -----------------------------------------------------------------

    fn replay_chain(heap: &mut Heap, ops: &[(Value, Value)]) -> *mut Table {
        let t = heap.new_table();
        let tref = unsafe { t.as_mut() };
        for (k, v) in ops.iter().copied() {
            tref.set(heap, k, v).unwrap();
        }
        t.as_ptr()
    }

    fn replay_soa(heap: &mut Heap, ops: &[(Value, Value)]) -> *mut Table {
        let t = heap.new_table();
        let tref = unsafe { t.as_mut() };
        for (k, v) in ops.iter().copied() {
            tref.soa_insert(heap, k, v).unwrap();
        }
        t.as_ptr()
    }

    #[test]
    fn c3_soa_equivalence_string_keys() {
        let mut heap = Heap::new();
        let mut ops = Vec::new();
        for i in 0..40 {
            let k = Value::Str(heap.intern(format!("key_{i:03}").as_bytes()));
            ops.push((k, Value::Int(i * 7)));
        }
        let chain = unsafe { &*replay_chain(&mut heap, &ops) };
        let soa = unsafe { &*replay_soa(&mut heap, &ops) };
        for (k, _) in &ops {
            let cv = chain.get(*k);
            let sv = soa.soa_get(*k);
            assert!(
                cv.raw_eq(sv),
                "SoA vs chain mismatch on key — chain={:?} soa={:?}",
                cv,
                sv,
            );
        }
        // Absent key returns nil from both paths.
        let absent = Value::Str(heap.intern(b"never"));
        assert!(chain.get(absent).is_nil());
        assert!(soa.soa_get(absent).is_nil());
    }

    #[test]
    fn c3_soa_equivalence_negative_int_keys() {
        // Dense negative ints with identity hashing — same collision
        // profile as the existing `collision_relocation_keeps_chains_intact`
        // test, but verified through the SoA RH path. Triggers
        // rob-from-rich repeatedly.
        let mut heap = Heap::new();
        let mut ops = Vec::new();
        for i in 0..256 {
            let k = Value::Int(-i);
            ops.push((k, Value::Int(i)));
        }
        let chain = unsafe { &*replay_chain(&mut heap, &ops) };
        let soa = unsafe { &*replay_soa(&mut heap, &ops) };
        for (k, _) in &ops {
            let cv = chain.get(*k);
            let sv = soa.soa_get(*k);
            assert!(cv.raw_eq(sv), "SoA mismatch on key {:?}", k);
        }
    }

    #[test]
    fn c3_soa_equivalence_mixed_keys_with_updates() {
        // Insert, then update the same keys with new values — exercises
        // the soa_find_slot in-place update branch.
        let mut heap = Heap::new();
        let kstr = Value::Str(heap.intern(b"x"));
        let kint = Value::Int(42);
        let kbool = Value::Bool(true);
        let ops: Vec<(Value, Value)> = vec![
            (kstr, Value::Int(1)),
            (kint, Value::Int(2)),
            (kbool, Value::Int(3)),
            (kstr, Value::Int(11)),  // update
            (kint, Value::Int(22)),  // update
            (kbool, Value::Int(33)), // update
        ];
        let chain = unsafe { &*replay_chain(&mut heap, &ops) };
        let soa = unsafe { &*replay_soa(&mut heap, &ops) };
        for k in [kstr, kint, kbool] {
            assert!(chain.get(k).raw_eq(soa.soa_get(k)));
        }
    }

    #[test]
    fn c3_soa_equivalence_delete_then_read() {
        // Phase D: tombstone delete + read on both paths, verify
        // matching nil-for-deleted, original-val-for-live.
        let mut heap = Heap::new();
        let mut ops_insert = Vec::new();
        for i in 0..30 {
            let k = Value::Str(heap.intern(format!("d_key_{i:03}").as_bytes()));
            ops_insert.push((k, Value::Int(i * 11)));
        }
        let chain = unsafe { &mut *replay_chain(&mut heap, &ops_insert) };
        let soa = unsafe { &mut *replay_soa(&mut heap, &ops_insert) };
        // Delete every 3rd key.
        let mut deleted: Vec<Value> = Vec::new();
        for (i, (k, _)) in ops_insert.iter().enumerate() {
            if i % 3 == 0 {
                // chain: set to Nil is the chain-path's delete equivalent
                chain.set(&mut heap, *k, Value::Nil).unwrap();
                let was_present = soa.soa_delete(*k);
                assert!(was_present, "soa_delete miss on inserted key {:?}", k);
                deleted.push(*k);
            }
        }
        // Read each key: deleted → nil, non-deleted → original val.
        for (k, v) in &ops_insert {
            let cv = chain.get(*k);
            let sv = soa.soa_get(*k);
            assert!(
                cv.raw_eq(sv),
                "delete/read mismatch on key {:?} — chain={:?} soa={:?}",
                k,
                cv,
                sv,
            );
            if deleted.iter().any(|d| d.raw_eq(*k)) {
                assert!(cv.is_nil(), "deleted key {:?} chain non-nil", k);
                assert!(sv.is_nil(), "deleted key {:?} soa non-nil", k);
            } else {
                assert!(cv.raw_eq(*v), "live key {:?} chain val drift", k);
            }
        }
        // Deleting an absent key is a no-op (returns false) on SoA.
        let absent = Value::Str(heap.intern(b"never_d"));
        assert!(!soa.soa_delete(absent));
    }

    #[test]
    fn c3_soa_delete_then_reinsert_uses_tombstone() {
        // After delete + reinsert, key is findable with new val. The
        // SoA path may reuse the tombstoned slot (preferred) or place
        // elsewhere — either is correct as long as soa_get returns
        // the new val.
        let mut heap = Heap::new();
        let t = heap.new_table();
        let tref = unsafe { t.as_mut() };
        let k = Value::Str(heap.intern(b"reinsert_target"));
        tref.soa_insert(&mut heap, k, Value::Int(100)).unwrap();
        assert!(tref.soa_get(k).raw_eq(Value::Int(100)));
        let pre_tombs = tref.tombstones;
        assert!(tref.soa_delete(k));
        assert!(tref.tombstones == pre_tombs + 1);
        assert!(tref.soa_get(k).is_nil());
        // Reinsert with new val.
        tref.soa_insert(&mut heap, k, Value::Int(200)).unwrap();
        assert!(tref.soa_get(k).raw_eq(Value::Int(200)));
        // Tombstone reused — count back to pre_tombs.
        assert_eq!(tref.tombstones, pre_tombs);
    }

    #[test]
    fn c3_soa_grows_under_load_pressure() {
        // Stress test: insert enough entries to trigger multiple RH
        // rehashes (cap doubles at load 0.75). Confirms PSL overflow
        // never fires and all keys survive grow cycles.
        let mut heap = Heap::new();
        let t = heap.new_table();
        let tref = unsafe { t.as_mut() };
        for i in 0..1024 {
            let k = Value::Str(heap.intern(format!("entry_{i:05}").as_bytes()));
            tref.soa_insert(&mut heap, k, Value::Int(i)).unwrap();
        }
        // Verify every key is findable.
        for i in 0..1024 {
            let k = Value::Str(heap.intern(format!("entry_{i:05}").as_bytes()));
            let v = tref.soa_get(k);
            assert!(
                v.raw_eq(Value::Int(i)),
                "SoA lost key entry_{:05} — got {:?}",
                i,
                v,
            );
        }
        assert!(tref.soa_live_count() == 1024);
        // Cap should have grown past the initial SOA_INITIAL_CAP via
        // the 0.75 load-factor trigger.
        assert!(
            tref.soa_cap() >= 2048,
            "SoA cap = {} after 1024 inserts — load gate didn't grow",
            tref.soa_cap(),
        );
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
