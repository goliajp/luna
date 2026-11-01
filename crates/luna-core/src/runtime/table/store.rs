//! The stores into a table, on its handle: each takes the write
//! barrier itself, so no store leaves a black table holding a white object.

use super::*;

impl Gc<Table> {
    /// The table, to write.
    #[inline(always)]
    fn table_mut<'a>(self) -> &'a mut Table {
        // SAFETY: these stores are crate-internal (luna's other crates reach
        // them as the `unsafe fn`s of `__private::TableStores`): their callers
        // pass a live table of the heap they pass, and hold no other reference
        // to it across the store, which runs no collection
        unsafe { self.as_mut() }
    }

    /// Insert / update `(key, val)`, with the write barrier. `heap` counts
    /// a rehash's parts (see `runtime::puc`).
    /// A read-only table refuses the write with [`TableError::ReadOnly`].
    pub(crate) fn set(self, heap: &mut Heap, key: Value, val: Value) -> Result<(), TableError> {
        let t = self.table_mut();
        if t.is_readonly() {
            return Err(TableError::ReadOnly);
        }
        t.set_inlined(heap, key, val)?;
        heap.barrier_store(self, &[key, val]);
        Ok(())
    }

    /// Integer-keyed variant of [`Gc::<Table>::set`], with the write barrier.
    pub(crate) fn set_int(self, heap: &mut Heap, i: i64, val: Value) -> Result<(), TableError> {
        let t = self.table_mut();
        if t.is_readonly() {
            return Err(TableError::ReadOnly);
        }
        t.set_int_raw(heap, i, val)?;
        heap.barrier_store(self, &[val]);
        Ok(())
    }

    /// Overwrite `key`'s value when the key is already there with a value
    /// that is not nil (no new key, so no rehash and no `__newindex`):
    /// whether it stored, taking the write barrier when it did. A
    /// read-only table stores nothing.
    #[inline]
    pub(crate) fn try_set_existing(self, heap: &mut Heap, key: Value, val: Value) -> bool {
        let t = self.table_mut();
        let stored = !t.is_readonly() && t.set_existing_raw(key, val);
        if stored {
            heap.barrier_store(self, &[val]);
        }
        stored
    }

    /// Install (or clear) this table's metatable, with the write barrier
    /// each dialect's `lua_setmetatable` takes. Does not perform any
    /// `__metatable` guarding; that belongs in the Vm-level `setmetatable`.
    pub(crate) fn set_metatable(self, heap: &mut Heap, mt: Option<Gc<Table>>) {
        self.table_mut().metatable = mt;
        if let Some(m) = mt {
            heap.metatable_stored(self, m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_table_refuses_every_store() {
        let mut heap = Heap::new();
        let t = heap.new_table();
        let k = Value::Str(heap.intern(b"k"));
        t.set(&mut heap, k, Value::Int(1)).unwrap();
        // SAFETY: `t` is held only by this local and no collection runs
        unsafe { t.as_mut() }.set_readonly(true);
        assert_eq!(
            t.set(&mut heap, k, Value::Int(2)),
            Err(TableError::ReadOnly)
        );
        assert_eq!(
            t.set_int(&mut heap, 1, Value::Int(2)),
            Err(TableError::ReadOnly)
        );
        assert!(!t.try_set_existing(&mut heap, k, Value::Int(2)));
        assert!(t.get(k).raw_eq(Value::Int(1)));
        assert!(t.get_int(1).is_nil());
    }
}
