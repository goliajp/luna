//! GC heap v1: precise stop-the-world mark & sweep over an intrusive
//! all-objects list (PUC `allgc` shape). All unsafe object plumbing is
//! confined to this module and `string`/`table` internals.
//!
//! `Gc<T>` safety contract: the runtime is single-threaded; a `Gc` pointer is
//! valid until a `collect()` call that does not reach it from the given
//! roots. Callers must root every value they keep across a collect.

use std::fmt;
use std::ops::Deref;
use std::ptr::{self, NonNull};

use crate::runtime::string::{self, LuaStr, StringTable};
use crate::runtime::table::Table;
use crate::runtime::value::Value;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ObjTag {
    Str,
    Table,
}

#[repr(C)]
pub struct GcHeader {
    next: *mut GcHeader,
    tag: ObjTag,
    /// bit 0: mark; remaining bits reserved for tri-color + age (P06)
    flags: u8,
}

const MARK: u8 = 1;

impl GcHeader {
    pub(crate) fn new(tag: ObjTag) -> GcHeader {
        GcHeader {
            next: ptr::null_mut(),
            tag,
            flags: 0,
        }
    }
}

pub struct Gc<T> {
    ptr: NonNull<T>,
}

impl<T> Clone for Gc<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Gc<T> {}

impl<T> Gc<T> {
    pub(crate) fn from_ptr(p: *mut T) -> Gc<T> {
        Gc {
            ptr: NonNull::new(p).expect("gc pointer must be non-null"),
        }
    }

    pub fn as_ptr(self) -> *mut T {
        self.ptr.as_ptr()
    }

    pub fn ptr_eq(self, other: Gc<T>) -> bool {
        self.ptr == other.ptr
    }

    /// SAFETY: caller must ensure no other live reference to the object and
    /// no collect() while the borrow is held (single-threaded runtime).
    #[allow(dead_code)] // used by tests now; the VM mutation path lands in P03
    pub(crate) unsafe fn as_mut<'a>(self) -> &'a mut T {
        unsafe { &mut *self.ptr.as_ptr() }
    }
}

impl<T> Deref for Gc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gc({:p})", self.ptr.as_ptr())
    }
}

pub struct Heap {
    all: *mut GcHeader,
    strings: StringTable,
    seed: u32,
    live: usize,
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            all: ptr::null_mut(),
            strings: StringTable::new(),
            seed: make_seed(),
            live: 0,
        }
    }

    fn link(&mut self, h: *mut GcHeader) {
        unsafe {
            (*h).next = self.all;
        }
        self.all = h;
        self.live += 1;
    }

    pub fn new_table(&mut self) -> Gc<Table> {
        let p = Box::into_raw(Box::new(Table::new(GcHeader::new(ObjTag::Table))));
        self.link(p as *mut GcHeader);
        Gc::from_ptr(p)
    }

    /// Create (or find) a string. Short strings (≤ 40 bytes) are interned.
    pub fn intern(&mut self, bytes: &[u8]) -> Gc<LuaStr> {
        if bytes.len() <= string::MAX_SHORT_LEN {
            let (p, is_new) = self.strings.intern(bytes, self.seed);
            if is_new {
                self.link(p as *mut GcHeader);
            }
            Gc::from_ptr(p)
        } else {
            let p = string::alloc_long(bytes, self.seed);
            self.link(p as *mut GcHeader);
            Gc::from_ptr(p)
        }
    }

    pub fn live_objects(&self) -> usize {
        self.live
    }

    /// Forward write barrier hook (no-op until incremental GC in P06).
    #[inline(always)]
    pub fn barrier_forward(&mut self, _parent: Value, _child: Value) {}

    /// Mark from `roots`, sweep everything unreachable. Returns the number of
    /// objects freed.
    pub fn collect(&mut self, roots: &[Value]) -> usize {
        let mut stack: Vec<*mut GcHeader> = Vec::new();
        for &r in roots {
            mark_value(r, &mut stack);
        }
        while let Some(h) = stack.pop() {
            unsafe {
                if (*h).tag == ObjTag::Table {
                    let t = &*(h as *mut Table);
                    t.trace(&mut |v| mark_value(v, &mut stack));
                }
            }
        }
        // sweep: detach the list first so freeing (which needs &mut self for
        // the string table) never aliases a pointer into self
        let mut freed = 0;
        unsafe {
            let mut cur = std::mem::replace(&mut self.all, ptr::null_mut());
            let mut kept_head: *mut GcHeader = ptr::null_mut();
            let mut kept_tail: *mut GcHeader = ptr::null_mut();
            while !cur.is_null() {
                let next = (*cur).next;
                if (*cur).flags & MARK != 0 {
                    (*cur).flags &= !MARK;
                    (*cur).next = ptr::null_mut();
                    if kept_tail.is_null() {
                        kept_head = cur;
                    } else {
                        (*kept_tail).next = cur;
                    }
                    kept_tail = cur;
                } else {
                    self.free_obj(cur);
                    freed += 1;
                }
                cur = next;
            }
            self.all = kept_head;
        }
        self.live -= freed;
        freed
    }

    unsafe fn free_obj(&mut self, h: *mut GcHeader) {
        unsafe {
            match (*h).tag {
                ObjTag::Table => drop(Box::from_raw(h as *mut Table)),
                ObjTag::Str => {
                    let s = h as *mut LuaStr;
                    if (*s).is_short() {
                        self.strings.remove(s);
                    }
                    string::free(s);
                }
            }
        }
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // free everything regardless of reachability
        unsafe {
            let mut cur = self.all;
            while !cur.is_null() {
                let next = (*cur).next;
                self.free_obj(cur);
                cur = next;
            }
        }
    }
}

impl Default for Heap {
    fn default() -> Heap {
        Heap::new()
    }
}

fn mark_value(v: Value, stack: &mut Vec<*mut GcHeader>) {
    let h = match v {
        Value::Str(s) => s.as_ptr() as *mut GcHeader,
        Value::Table(t) => t.as_ptr() as *mut GcHeader,
        _ => return,
    };
    unsafe {
        if (*h).flags & MARK == 0 {
            (*h).flags |= MARK;
            stack.push(h);
        }
    }
}

/// Hash seed from address entropy (ASLR) and clock, luaL_makeseed style.
fn make_seed() -> u32 {
    let stack_var = 0u8;
    let mut h = &stack_var as *const u8 as u64;
    h ^= (make_seed as *const () as u64) << 16;
    if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h ^= (d.subsec_nanos() as u64) << 32 ^ d.as_secs();
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_unreachable() {
        let mut heap = Heap::new();
        let s = heap.intern(b"hello");
        let t = heap.new_table();
        assert_eq!(heap.live_objects(), 2);
        // both rooted: nothing freed
        assert_eq!(heap.collect(&[Value::Str(s), Value::Table(t)]), 0);
        // only table rooted: string freed
        assert_eq!(heap.collect(&[Value::Table(t)]), 1);
        assert_eq!(heap.live_objects(), 1);
        // nothing rooted
        assert_eq!(heap.collect(&[]), 1);
        assert_eq!(heap.live_objects(), 0);
    }

    #[test]
    fn collect_traces_table_contents() {
        let mut heap = Heap::new();
        let t = heap.new_table();
        let k = heap.intern(b"key-string-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // long
        let v = heap.intern(b"val");
        unsafe { t.as_mut() }
            .set(Value::Str(k), Value::Str(v))
            .unwrap();
        let inner = heap.new_table();
        unsafe { t.as_mut() }
            .set(Value::Int(1), Value::Table(inner))
            .unwrap();
        unsafe { inner.as_mut() }.set_metatable(Some(t));
        assert_eq!(heap.live_objects(), 4);
        // root only the outer table: everything reachable through it survives
        assert_eq!(heap.collect(&[Value::Table(t)]), 0);
        assert_eq!(heap.live_objects(), 4);
        assert_eq!(heap.collect(&[]), 4);
    }

    #[test]
    fn interned_string_reclaimed_and_reinternable() {
        let mut heap = Heap::new();
        heap.intern(b"transient");
        assert_eq!(heap.collect(&[]), 1);
        let s2 = heap.intern(b"transient");
        assert_eq!(s2.as_bytes(), b"transient");
        assert_eq!(heap.live_objects(), 1);
    }

    #[test]
    fn deep_table_chain_marks_iteratively() {
        // deep chain: explicit mark stack must not overflow (smaller under
        // miri — the interpreter makes 100k tables take ~30 minutes)
        let n = if cfg!(miri) { 2_000 } else { 100_000 };
        let mut heap = Heap::new();
        let head = heap.new_table();
        let mut cur = head;
        for _ in 0..n {
            let next = heap.new_table();
            unsafe { cur.as_mut() }
                .set(Value::Int(1), Value::Table(next))
                .unwrap();
            cur = next;
        }
        assert_eq!(heap.collect(&[Value::Table(head)]), 0);
        assert_eq!(heap.collect(&[]), n + 1);
    }
}
