//! Lua strings: immutable byte sequences allocated in one block (header +
//! inline bytes). Short strings (≤ 40 bytes, PUC LUAI_MAXSHORTLEN) are
//! interned in the heap's string table: equality is pointer equality. Long
//! strings hash lazily, seeded per-heap (hash-flooding defense for hostile
//! script workloads (script host)).

use std::alloc::Layout;

use crate::runtime::mem::{BlockKind, MemRef};
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
    // `hdr.aux` is the byte length; that many bytes follow the struct
}

impl LuaStr {
    /// Byte length of the string (not character count).
    pub fn len(&self) -> usize {
        self.hdr.aux as usize
    }

    /// True when the string is zero bytes long.
    pub fn is_empty(&self) -> bool {
        self.hdr.aux == 0
    }

    pub(crate) fn is_short(&self) -> bool {
        self.short
    }
}

/// Field offsets the JIT reads from a `LuaStr` in compiled code.
pub mod jit_layout {
    use super::LuaStr;

    /// Byte offset of the `bool` that is true exactly for interned
    /// (short) strings. Two distinct interned strings are unequal.
    pub const STR_SHORT_OFFSET: usize = std::mem::offset_of!(LuaStr, short);
    /// Byte offset of the `u32` hash (a short string's is set when it is
    /// interned): its main position in a hash part is `hash & node_mask`.
    pub const STR_HASH_OFFSET: usize = std::mem::offset_of!(LuaStr, hash);
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

    /// The bytes as a C string (PUC `getstr`): a NUL follows them in the
    /// same allocation, so C can read the string up to its first NUL; the
    /// pointer is valid while the string is alive.
    pub fn as_c_ptr(&self) -> *const std::ffi::c_char {
        self.as_bytes().as_ptr().cast()
    }

    /// The hash field as it stands: a short string's hash, a long string's
    /// hash or, before [`Self::hash`] computed it, the heap seed.
    #[inline(always)]
    pub(crate) fn stored_hash(&self) -> u32 {
        // SAFETY: `self.as_ptr()` is a live string header
        unsafe { (*self.as_ptr()).hash.get() }
    }

    /// Cached hash of the string (computed lazily for long strings).
    #[inline]
    pub fn hash(&self) -> u32 {
        // SAFETY: `self.as_ptr()` is the start of this `LuaStr`'s header which was allocated with the trailing bytes / hash fields in the same allocation by `StringTable::intern`.
        unsafe { hash_of(self.as_ptr()) }
    }
}

/// The bytes of the string at `p`.
///
/// # Safety
/// `p` points to a live string allocation (with its tail), and the string
/// is not freed while the returned slice is in use.
pub(crate) unsafe fn bytes_of<'a>(p: *const LuaStr) -> &'a [u8] {
    // SAFETY: the caller's contract; the bytes follow the header in the
    // same allocation, and `hdr.aux` holds their count
    unsafe { slice::from_raw_parts(p.add(1) as *const u8, (*p).hdr.aux as usize) }
}

/// The hash of the string at `p`, computed and cached on first use.
///
/// # Safety
/// `p` points to a live string allocation (with its tail).
#[inline]
pub(crate) unsafe fn hash_of(p: *const LuaStr) -> u32 {
    // SAFETY: the caller's contract
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

/// PUC 5.1 `luaS_newlstr`'s hash: seeded with the length, not per run, and
/// on a long string only every `len / 32 + 1`-th byte from the end. 5.1
/// places string keys by it, so a 5.1 state uses it to place them as PUC
/// does.
pub(crate) fn lua_hash_51(bytes: &[u8]) -> u32 {
    let l = bytes.len();
    let mut h = l as u32;
    let step = (l >> 5) + 1;
    let mut l1 = l;
    while l1 >= step {
        h ^= h
            .wrapping_shl(5)
            .wrapping_add(h >> 2)
            .wrapping_add(u32::from(bytes[l1 - 1]));
        l1 -= step;
    }
    h
}

/// Longest string a `LuaStr` can describe (its length is a `u32`). Code
/// that builds a string from script-controlled pieces checks against it.
pub(crate) const MAX_LEN: usize = u32::MAX as usize;

// one byte more than the string for a terminating NUL, so the bytes can be
// handed to C as a C string, as PUC's strings can
fn layout(len: usize) -> Layout {
    Layout::new::<LuaStr>()
        .extend(Layout::array::<u8>(len + 1).expect("string size overflows layout"))
        .expect("string size overflows layout")
        .0
        .pad_to_align()
}

fn alloc_str(mem: MemRef, bytes: &[u8], short: bool, hash: u32, hashed: bool) -> *mut LuaStr {
    let layout = layout(bytes.len());
    let p = match mem.ctx().alloc(layout, BlockKind::Str) {
        Some(p) => p.as_ptr() as *mut LuaStr,
        None => crate::runtime::mem::oom_abort(layout),
    };
    // SAFETY: layout is built from the header size + the trailing bytes and their NUL that we just computed, so the header, the bytes and the NUL written below are inside the allocation; deallocation will use the same layout in `free`.
    unsafe {
        let mut hdr = GcHeader::new(ObjTag::Str);
        hdr.aux = bytes.len() as u32;
        p.write(LuaStr {
            hdr,
            hnext: ptr::null_mut(),
            hash: Cell::new(hash),
            hashed: Cell::new(hashed),
            short,
        });
        ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(1) as *mut u8, bytes.len());
        (p.add(1) as *mut u8).add(bytes.len()).write(0);
        p
    }
}

/// A long string: hashed when first used as a key, or at once with PUC
/// 5.1's hash (`h51`).
pub(crate) fn alloc_long(mem: MemRef, bytes: &[u8], seed: u32, h51: bool) -> *mut LuaStr {
    debug_assert!(bytes.len() > MAX_SHORT_LEN);
    if h51 {
        alloc_str(mem, bytes, false, lua_hash_51(bytes), true)
    } else {
        alloc_str(mem, bytes, false, seed, false)
    }
}

/// SAFETY: `p` must come from `alloc_str` on `mem` and not be freed twice.
pub(crate) unsafe fn free(p: *mut LuaStr, mem: MemRef) {
    // SAFETY: the caller's contract; the layout is the one `alloc_str`
    // used, recomputed from the byte count in `hdr.aux`
    unsafe {
        let l = layout((*p).hdr.aux as usize);
        ptr::drop_in_place(p);
        mem.ctx().free(ptr::NonNull::new_unchecked(p as *mut u8), l);
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

    pub(crate) fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Find or create an interned short string. Returns `(ptr, newly_created)`.
    #[inline]
    pub(crate) fn intern(
        &mut self,
        mem: MemRef,
        bytes: &[u8],
        seed: u32,
        h51: bool,
    ) -> (*mut LuaStr, bool) {
        debug_assert!(bytes.len() <= MAX_SHORT_LEN);
        let h = if h51 {
            lua_hash_51(bytes)
        } else {
            lua_hash(bytes, seed)
        };
        let b = h as usize & (self.buckets.len() - 1);
        let mut cur = self.buckets[b];
        // SAFETY: `self.as_ptr()` is the start of this `LuaStr`'s header which was allocated with the trailing bytes / hash fields in the same allocation by `StringTable::intern`.
        unsafe {
            while !cur.is_null() {
                if (*cur).hdr.aux as usize == bytes.len() && bytes_of(cur) == bytes {
                    return (cur, false);
                }
                cur = (*cur).hnext;
            }
        }
        if self.count >= self.buckets.len() {
            self.grow();
        }
        let b = h as usize & (self.buckets.len() - 1);
        let p = alloc_str(mem, bytes, true, h, true);
        // SAFETY: `p` was just returned by `alloc_str` and is not in any chain yet; nothing else points at it
        unsafe {
            (*p).hnext = self.buckets[b];
        }
        self.buckets[b] = p;
        self.count += 1;
        (p, true)
    }

    #[cold]
    #[inline(never)]
    fn grow(&mut self) {
        let mut nb = vec![ptr::null_mut(); self.buckets.len() * 2];
        let mask = nb.len() - 1;
        for &head in &self.buckets {
            let mut cur = head;
            while !cur.is_null() {
                // SAFETY: the bucket chains hold only interned strings that are still allocated: `remove` unlinks a string before the sweep frees it
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
    ///
    /// # Safety
    /// `p` is a short string this table interned, still allocated and in its
    /// bucket.
    pub(crate) unsafe fn remove(&mut self, p: *mut LuaStr) {
        // SAFETY: `p` is allocated and chained (the caller's contract), and every other chained string is allocated too, so `cur` always points at a bucket slot or at the `hnext` of a live string
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
