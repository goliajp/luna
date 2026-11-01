//! Heaps, and the stores and barriers that must be given the heap that
//! allocated the object they write: luna's other crates and its tests.

use crate::runtime::heap::{Gc, GcObject};
use crate::runtime::{Heap, Table, TableError, Value};

/// A heap of its own, with a random string hash seed.
pub fn new_heap() -> Heap {
    Heap::new()
}

/// The write barriers of `Heap` luna's other crates use.
pub trait HeapBarriers {
    /// PUC `luaC_barrier_`: `parent` was given a reference to `child`.
    ///
    /// # Safety
    /// `parent`, and `child` when it is an object, are live objects of
    /// this heap.
    unsafe fn barrier_forward<T: GcObject>(&mut self, parent: Gc<T>, child: Value);
    /// PUC `luaC_barrierback_`: `parent` is about to be given references.
    ///
    /// # Safety
    /// `parent` is a live object of this heap.
    unsafe fn barrier_back<T: GcObject>(&mut self, parent: Gc<T>);
    /// The write barrier a table or userdata `t` takes for `vals` just
    /// stored into it, as the interpreter's stores take it.
    ///
    /// # Safety
    /// `t`, and each of `vals` that is an object, are live objects of this
    /// heap.
    unsafe fn barrier_store<T: GcObject>(&mut self, t: Gc<T>, vals: &[Value]);
}

impl HeapBarriers for Heap {
    #[inline(always)]
    unsafe fn barrier_forward<T: GcObject>(&mut self, parent: Gc<T>, child: Value) {
        Heap::barrier_forward(self, parent, child)
    }
    #[inline(always)]
    unsafe fn barrier_back<T: GcObject>(&mut self, parent: Gc<T>) {
        Heap::barrier_back(self, parent)
    }
    #[inline(always)]
    unsafe fn barrier_store<T: GcObject>(&mut self, t: Gc<T>, vals: &[Value]) {
        Heap::barrier_store(self, t, vals)
    }
}

/// The stores into a table luna's other crates make, each with the write
/// barrier; an embedder writes through `Vm::raw_set` or `TableBuilder`.
///
/// # Safety
/// For every method: `heap` is the heap that allocated the table, and the
/// key and value objects are live objects of that heap; the table is live;
/// no other reference to the table (a `&Table` through `Deref` included)
/// is held across the call.
pub trait TableStores {
    /// Store `(key, val)`; a read-only table refuses it.
    ///
    /// # Safety
    /// See the trait.
    unsafe fn store(self, heap: &mut Heap, key: Value, val: Value) -> Result<(), TableError>;
    /// Store `val` at integer key `i`; a read-only table refuses it.
    ///
    /// # Safety
    /// See the trait.
    unsafe fn store_int(self, heap: &mut Heap, i: i64, val: Value) -> Result<(), TableError>;
    /// Overwrite `key`'s value when the key holds one already: whether it
    /// stored. A read-only table stores nothing.
    ///
    /// # Safety
    /// See the trait.
    unsafe fn store_existing(self, heap: &mut Heap, key: Value, val: Value) -> bool;
    /// Install or clear the table's metatable, without `__metatable` tests.
    ///
    /// # Safety
    /// See the trait.
    unsafe fn store_metatable(self, heap: &mut Heap, mt: Option<Gc<Table>>);
}

impl TableStores for Gc<Table> {
    #[inline]
    unsafe fn store(self, heap: &mut Heap, key: Value, val: Value) -> Result<(), TableError> {
        self.set(heap, key, val)
    }
    #[inline]
    unsafe fn store_int(self, heap: &mut Heap, i: i64, val: Value) -> Result<(), TableError> {
        self.set_int(heap, i, val)
    }
    #[inline]
    unsafe fn store_existing(self, heap: &mut Heap, key: Value, val: Value) -> bool {
        self.try_set_existing(heap, key, val)
    }
    #[inline]
    unsafe fn store_metatable(self, heap: &mut Heap, mt: Option<Gc<Table>>) {
        self.set_metatable(heap, mt)
    }
}
