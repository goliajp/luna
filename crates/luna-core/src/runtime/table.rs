//! Lua table: hybrid array + hash.
//!
//! Array part uses split tag/payload storage (9 bytes/slot — the Lua 5.5
//! "compact arrays" layout, bench-validated in benches/value_repr.rs).
//! Hash part is the PUC node layout: main-position chaining with relocation
//! (Brent's variation), capacity a power of two, rehash sizing per
//! luaH_rehash/computesizes.

use crate::runtime::heap::{Gc, GcHeader, Heap};
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

/// JIT layout constants for table-field IC.
///
/// luna-jit's trace lowerer needs to emit direct loads against
/// `Table.nodes` (the hash part) without paying the helper-call ABI
/// for each `Op::GetField` / `Op::SetField`. These constants expose
/// the field offsets so the cranelift IR can be parameterised at
/// compile time. The `Node` struct itself remains `pub(crate)` — only
/// the offsets cross the crate boundary.
///
/// Layout assumptions:
/// - the hash part is a node pointer plus a `u32` mask (node count - 1,
///   `u64::MAX` when empty); the unit test `node_layout_pinned` reads
///   both at their offsets.
/// - `Value` is `#[repr(C, u8)]` so the discriminant byte sits at
///   offset 0 and the payload starts at offset 8 (after 7 bytes of
///   alignment padding). Total size 16 bytes per the existing
///   `value_is_16_bytes` test in `runtime/value.rs`.
/// - `Node` is `#[repr(C)]`: the key's tag at offset 0 and its payload
///   at 8, where a `Value` keeps them, `dead_key` and `next` in the
///   bytes between (a `Value`'s padding), and `val` at offset 16; 32
///   bytes in all. `dead_key` and `next` are not read by the IC.
#[path = "table_jit_layout.rs"]
pub mod jit_layout;

#[path = "table_array.rs"]
mod array;
#[path = "table_get.rs"]
mod get;
#[path = "table_grow.rs"]
mod grow;
#[path = "table_node.rs"]
mod node;
#[path = "table_resize.rs"]
mod resize;
#[path = "table_set.rs"]
mod set;
#[path = "table_slab.rs"]
mod slab;
#[path = "table_trace.rs"]
mod trace;
#[path = "table_walk.rs"]
mod walk;
#[path = "table_weak.rs"]
mod weak;
use node::{NONE, Node};

/// Inline storage threshold. Tables whose array part has
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
    /// read through raw casts by the GC; its `aux`
    /// word holds the absent-metamethod bits (PUC `flags`): bit `1 << Mm`
    /// set means this table, used as a metatable, has no such field. Set
    /// by the lookup on a miss; cleared whenever a hash key gains a value
    /// (`set_norm`, `insert_new`)
    pub(crate) hdr: GcHeader,
    /// Single backing pointer for the array part. Points to
    /// `inline_storage` (asize <= INLINE_ASIZE) or to an external slab
    /// this table owns (asize > INLINE_ASIZE, freed by `Drop`). The JIT
    /// inline aset reads this with one `load i64`, no branch — the choice
    /// between inline and slab is already encoded in the pointer.
    /// Initialised in `Heap::new_table` AFTER the Table reaches its final
    /// heap address (so that `&mut self.inline_storage` is the stable heap
    /// pointer, not a stack-local one). Updated by `Table::resize`.
    pub array_ptr: *mut u8,
    /// Length of the array part in slots. u64 (rather than `usize` or
    /// `u32`) so the JIT can load it with a single `load i64`.
    pub asize: u64,
    /// hash part: `node_mask + 1` nodes (a power of two), or none. Owned:
    /// it is a leaked `Box<[Node]>` of that length (dangling when empty),
    /// taken back by `take_hash_part`; the length lives in `node_mask` only
    nodes: *mut Node,
    /// Visible outside the module so the JIT can
    /// take its field offset at compile time and emit an inline
    /// "metatable.is_none()" guard before the inline aget fast path.
    /// `Option<Gc<Table>>` is 8 bytes via the NonNull-pointer-opt: 0
    /// ⇔ None, non-zero ⇔ Some.
    pub metatable: Option<Gc<Table>>,
    /// node count - 1, or `u32::MAX` (top bit set) when there are no
    /// nodes: a string probe masks with it and tests its top bit, without
    /// first deriving the mask from the length
    pub(crate) node_mask: u32,
    /// free-slot search position, counts down (PUC lastfree).
    /// `pub(crate)` so `Heap::new_table` can reset on pool recycle.
    pub(crate) lastfree: u32,
    /// Non-nil slots in the array part. With `aprefix` it answers `#t`
    /// without a search when the array holds exactly a leading run
    /// (`acount == aprefix < asize`: then `aprefix` is the only border
    /// there, the one the binary search in `len` finds). Kept by `aset`,
    /// `clear_weak` and `resize`; the method JIT's inline array stores
    /// keep it too.
    pub(crate) acount: u32,
    /// A length whose leading slots `[0, aprefix)` are all non-nil; it may
    /// lag behind the real run (that only disables the `#t` shortcut)
    /// but never exceeds it.
    pub(crate) aprefix: u32,
    /// Inline backing used when `asize <= INLINE_ASIZE`.
    /// Same layout as the slab: avals at low addresses (`asize * 8`
    /// bytes from offset 0), atags at the trailing `asize` bytes.
    ///
    /// `UnsafeCell` because `array_ptr` is a SELF-REFERENTIAL cached
    /// pointer into this field. Under Stacked Borrows, every
    /// `&mut self` method call's function-entry retag re-tags the
    /// whole `*self` byte range Unique and would pop the cached
    /// pointer's tag — subsequent `array_ptr` accesses would be UB (Miri:
    /// "retag ... tag does not exist in the borrow stack").
    /// An `UnsafeCell` region instead receives SharedReadWrite on
    /// retag, which coexists with the pointer derived from
    /// `UnsafeCell::get`. All reads/writes of the inline bytes MUST
    /// go through `array_ptr` / `.get()` — never through a direct
    /// `&`/`&mut` borrow of the array contents.
    pub(crate) inline_storage: std::cell::UnsafeCell<[u64; INLINE_U64S]>,
}

// SAFETY: `array_ptr` looks like an unprotected raw pointer field, but
// it always refers to memory the same Table owns (either its own inline
// storage or the slab it owns). The Table is heap-allocated and never
// moved post-adoption, so the pointer stays valid for the table's
// lifetime. No thread-unsafety concern: tables are accessed only
// through the Vm, single-threaded.
unsafe impl Send for Table {}
// SAFETY: as for `Send`
unsafe impl Sync for Table {}

// the sweep and the mark walk every table; keep it within a 96-byte
// allocation (PUC 5.4 `Table` is 56)
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Table>() == 88);

impl Drop for Table {
    fn drop(&mut self) {
        drop(self.take_hash_part());
        self.free_array_slab();
    }
}

impl Table {
    pub(crate) fn new(hdr: GcHeader) -> Table {
        Table {
            hdr,
            // `array_ptr` is fixed up in
            // `Heap::new_table` after the Table reaches its final heap
            // address (so that `&inline_storage` is the heap address,
            // not a stack-local one). Null sentinel here so a
            // bug-detection invariant flags any pre-fixup read.
            array_ptr: std::ptr::null_mut(),
            asize: 0,
            inline_storage: std::cell::UnsafeCell::new([0; INLINE_U64S]),
            nodes: std::ptr::NonNull::dangling().as_ptr(),
            node_mask: u32::MAX,
            lastfree: 0,
            acount: 0,
            metatable: None,
            aprefix: 0,
        }
    }

    /// The absent-metamethod bits (see `hdr`).
    #[inline(always)]
    pub(crate) fn absent_mm(&self) -> u32 {
        self.hdr.aux
    }

    /// Record that the metamethod behind `bit` is absent from `mt`. A
    /// lookup that finds nothing records it while it holds no reference
    /// into the table, so it takes the handle rather than `&mut self`.
    #[inline(always)]
    pub(crate) fn note_absent_mm(mt: Gc<Table>, bit: u32) {
        // SAFETY: a `Gc` handle points at a live object (see `Gc`); the
        // runtime is single-threaded, and no reference into `mt` is used
        // after this write
        unsafe { mt.as_mut() }.hdr.aux |= bit;
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
    /// Inline storage doesn't count toward this (it's part
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
        array_external + std::mem::size_of_val(self.nodes())
    }

    #[inline]
    fn asize(&self) -> usize {
        self.asize as usize
    }

    #[inline]
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

    #[inline]
    pub(crate) fn aset(&mut self, idx: usize, v: Value) {
        let (t, b) = v.unpack();
        // SAFETY: callers (`set_norm`, `set_int`) gate on
        // `idx < self.asize()`, and both slices are `asize` long. The two
        // `*_mut` calls each take a distinct `&mut self` borrow whose
        // lifetime ends at the statement boundary, so they don't overlap.
        let old = unsafe {
            let old = *self.atags().get_unchecked(idx);
            *self.atags_mut().get_unchecked_mut(idx) = t;
            *self.avals_mut().get_unchecked_mut(idx) = b;
            old
        };
        self.note_atag_change(idx, old, t);
    }
}

#[cfg(feature = "gc-verify")]
#[path = "table_gc_verify.rs"]
mod gc_verify;

#[inline]
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

#[inline]
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

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
