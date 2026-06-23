//! Lua strings: immutable byte sequences allocated in one block (header +
//! inline bytes). Short strings (≤ 40 bytes, PUC LUAI_MAXSHORTLEN) are
//! interned in the heap's string table: equality is pointer equality. Long
//! strings hash lazily, seeded per-heap (hash-flooding defense for hostile
//! script workloads (script host)).

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::cell::Cell;
use std::ptr;
use std::slice;

use crate::runtime::heap::{GcHeader, ObjTag};

/// Strings up to this byte length are interned in the heap's string table;
/// longer strings are heap-individual and hashed lazily.
pub const MAX_SHORT_LEN: usize = 40;

/// Lua string object — header plus inline byte payload. Byte-clean (Lua
/// strings are arbitrary byte sequences, not necessarily UTF-8). Access the
/// bytes via `Gc<LuaStr>::as_bytes`.
#[repr(C)]
pub struct LuaStr {
    pub(crate) hdr: GcHeader,
    /// string-table bucket chain (short strings only)
    hnext: *mut LuaStr,
    /// for long strings this holds the heap seed until the hash is computed
    hash: Cell<u32>,
    hashed: Cell<bool>,
    short: bool,
    len: u32,
    // `len` bytes follow the struct
}

impl LuaStr {
    /// Byte length of the string (not character count).
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// True when the string is zero bytes long.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn is_short(&self) -> bool {
        self.short
    }
}

/// Inline-bytes access MUST go through a pointer carrying the provenance of
/// the original allocation — a `&LuaStr` only covers the header, so deriving
/// the tail from it is UB (caught by miri). `Gc` stores the allocation
/// pointer, hence these live on `Gc<LuaStr>`.
impl crate::runtime::heap::Gc<LuaStr> {
    /// Borrow the underlying bytes of this Lua string.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: `self.as_ptr()` is the start of this `LuaStr`'s header which was allocated with the trailing bytes / hash fields in the same allocation by `StringTable::intern`.
        unsafe { bytes_of(self.as_ptr()) }
    }

    /// Cached hash of the string (computed lazily for long strings).
    pub fn hash(&self) -> u32 {
        // SAFETY: `self.as_ptr()` is the start of this `LuaStr`'s header which was allocated with the trailing bytes / hash fields in the same allocation by `StringTable::intern`.
        unsafe { hash_of(self.as_ptr()) }
    }
}

/// SAFETY: `p` must point to a live string allocation (with its tail).
pub(crate) unsafe fn bytes_of<'a>(p: *const LuaStr) -> &'a [u8] {
    unsafe { slice::from_raw_parts(p.add(1) as *const u8, (*p).len as usize) }
}

/// SAFETY: as `bytes_of`.
pub(crate) unsafe fn hash_of(p: *const LuaStr) -> u32 {
    unsafe {
        if !(*p).hashed.get() {
            (*p).hash.set(lua_hash(bytes_of(p), (*p).hash.get()));
            (*p).hashed.set(true);
        }
        (*p).hash.get()
    }
}

/// PUC luaS_hash (all bytes, no step — post-5.3 flooding fix).
pub(crate) fn lua_hash(bytes: &[u8], seed: u32) -> u32 {
    let mut h = seed ^ bytes.len() as u32;
    for &b in bytes {
        h ^= h
            .wrapping_shl(5)
            .wrapping_add(h.wrapping_shr(2))
            .wrapping_add(b as u32);
    }
    h
}

fn layout(len: usize) -> Layout {
    Layout::new::<LuaStr>()
        .extend(Layout::array::<u8>(len).expect("string size overflows layout"))
        .expect("string size overflows layout")
        .0
        .pad_to_align()
}

fn alloc_str(bytes: &[u8], short: bool, hash: u32, hashed: bool) -> *mut LuaStr {
    let layout = layout(bytes.len());
    // SAFETY: layout is built from the header size + trailing bytes length we just computed; deallocation will use the same layout in `Heap::sweep_strings`.
    unsafe {
        let p = alloc(layout) as *mut LuaStr;
        if p.is_null() {
            handle_alloc_error(layout);
        }
        p.write(LuaStr {
            hdr: GcHeader::new(ObjTag::Str),
            hnext: ptr::null_mut(),
            hash: Cell::new(hash),
            hashed: Cell::new(hashed),
            short,
            len: bytes.len() as u32,
        });
        ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(1) as *mut u8, bytes.len());
        p
    }
}

pub(crate) fn alloc_long(bytes: &[u8], seed: u32) -> *mut LuaStr {
    debug_assert!(bytes.len() > MAX_SHORT_LEN);
    alloc_str(bytes, false, seed, false)
}

/// SAFETY: `p` must come from `alloc_str` and not be freed twice.
pub(crate) unsafe fn free(p: *mut LuaStr) {
    unsafe {
        let l = layout((*p).len as usize);
        ptr::drop_in_place(p);
        dealloc(p as *mut u8, l);
    }
}

/// Open hashing with per-string chains (PUC stringtable shape).
pub(crate) struct StringTable {
    buckets: Vec<*mut LuaStr>,
    count: usize,
}

impl StringTable {
    pub(crate) fn new() -> StringTable {
        StringTable {
            buckets: vec![ptr::null_mut(); 64],
            count: 0,
        }
    }

    /// Find or create an interned short string. Returns `(ptr, newly_created)`.
    pub(crate) fn intern(&mut self, bytes: &[u8], seed: u32) -> (*mut LuaStr, bool) {
        debug_assert!(bytes.len() <= MAX_SHORT_LEN);
        let h = lua_hash(bytes, seed);
        let b = h as usize & (self.buckets.len() - 1);
        let mut cur = self.buckets[b];
        // SAFETY: `self.as_ptr()` is the start of this `LuaStr`'s header which was allocated with the trailing bytes / hash fields in the same allocation by `StringTable::intern`.
        unsafe {
            while !cur.is_null() {
                if (*cur).len as usize == bytes.len() && bytes_of(cur) == bytes {
                    return (cur, false);
                }
                cur = (*cur).hnext;
            }
        }
        if self.count >= self.buckets.len() {
            self.grow();
        }
        let b = h as usize & (self.buckets.len() - 1);
        let p = alloc_str(bytes, true, h, true);
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe {
            (*p).hnext = self.buckets[b];
        }
        self.buckets[b] = p;
        self.count += 1;
        (p, true)
    }

    fn grow(&mut self) {
        let mut nb = vec![ptr::null_mut(); self.buckets.len() * 2];
        let mask = nb.len() - 1;
        for &head in &self.buckets {
            let mut cur = head;
            while !cur.is_null() {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe {
                    let next = (*cur).hnext;
                    let b = (*cur).hash.get() as usize & mask;
                    (*cur).hnext = nb[b];
                    nb[b] = cur;
                    cur = next;
                }
            }
        }
        self.buckets = nb;
    }

    /// Unlink a dying interned string (called from sweep).
    pub(crate) fn remove(&mut self, p: *mut LuaStr) {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe {
            let b = (*p).hash.get() as usize & (self.buckets.len() - 1);
            let mut cur: *mut *mut LuaStr = &mut self.buckets[b];
            while !(*cur).is_null() {
                if *cur == p {
                    *cur = (*p).hnext;
                    self.count -= 1;
                    return;
                }
                cur = &mut (**cur).hnext;
            }
            unreachable!("interned string missing from string table");
        }
    }
}

/// Allocation footprint of a string of `len` bytes (heap accounting).
pub(crate) fn alloc_size(len: usize) -> usize {
    layout(len).size()
}

#[cfg(test)]
mod tests {
    use crate::runtime::heap::Heap;

    #[test]
    fn short_strings_are_interned() {
        let mut heap = Heap::new();
        let a = heap.intern(b"hello");
        let b = heap.intern(b"hello");
        let c = heap.intern(b"world");
        assert!(a.ptr_eq(b));
        assert!(!a.ptr_eq(c));
        assert_eq!(heap.live_objects(), 2);
        assert_eq!(a.as_bytes(), b"hello");
    }

    #[test]
    fn long_strings_are_not_interned() {
        let mut heap = Heap::new();
        let bytes = [0xAAu8; 64]; // non-UTF-8 long content
        let a = heap.intern(&bytes);
        let b = heap.intern(&bytes);
        assert!(!a.ptr_eq(b));
        assert_eq!(a.as_bytes(), b.as_bytes());
        // lazy hash agrees for equal content
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn arbitrary_bytes_roundtrip() {
        let mut heap = Heap::new();
        let bytes: Vec<u8> = (0..=255).collect();
        let s = heap.intern(&bytes);
        assert_eq!(s.as_bytes(), &bytes[..]);
        assert_eq!(s.len(), 256);
    }

    #[test]
    fn interning_survives_table_growth() {
        let mut heap = Heap::new();
        let first = heap.intern(b"key000");
        // push way past the initial 64 buckets to force grow()
        for i in 0..2000 {
            heap.intern(format!("key{i:03}").as_bytes());
        }
        let again = heap.intern(b"key000");
        assert!(first.ptr_eq(again));
    }
}
