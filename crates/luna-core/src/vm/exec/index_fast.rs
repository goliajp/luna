//! The table reads and writes the dispatch loop finishes itself, and the
//! hand-over to the metamethod chains for the rest.

use super::*;
use crate::runtime::string::LuaStr;
use crate::runtime::value::tag;
use fast_arith::{raw_gc, raw_int, raw_tag};

/// Raw `tb[*pk]` with the key read in place: strings by pointer, integers
/// in the array part first, other keys by the general lookup out of line.
///
/// # Safety
/// `pk` points at an initialised value.
#[inline(always)]
unsafe fn table_get_at(tb: &Table, pk: *const Value) -> Value {
    // SAFETY: the caller's contract; payloads are read after their tags
    unsafe {
        match raw_tag(pk) {
            tag::STR => {
                let key = Gc::from_ptr(raw_gc(pk) as *mut LuaStr);
                match tb.str_slot_by_ptr(key) {
                    Some(v) => *v,
                    None if key.is_short() => Value::Nil,
                    None => table_get_cold(tb, *pk),
                }
            }
            tag::INT => tb.get_int(raw_int(pk)),
            _ => table_get_cold(tb, *pk),
        }
    }
}

#[cold]
#[inline(never)]
fn table_get_cold(tb: &Table, key: Value) -> Value {
    tb.get(key)
}

/// Overwrite `tb[*pk]` with `v` when the key is present with a non-nil
/// value (PUC `luaV_fastset`); `false`, having done nothing, otherwise.
///
/// # Safety
/// `pk` points at an initialised value.
#[inline(always)]
unsafe fn table_set_existing_at(tb: &mut Table, pk: *const Value, v: Value) -> bool {
    // SAFETY: the caller's contract; payloads are read after their tags
    unsafe {
        match raw_tag(pk) {
            tag::STR => {
                let key = Gc::from_ptr(raw_gc(pk) as *mut LuaStr);
                match tb.str_slot_by_ptr_mut(key) {
                    // a nil value in a node is how a removed key is kept
                    Some(slot) if !slot.is_nil() => {
                        *slot = v;
                        true
                    }
                    Some(_) => false,
                    None if key.is_short() => false,
                    None => table_set_existing_cold(tb, *pk, v),
                }
            }
            tag::INT => tb.try_set_existing(Value::Int(raw_int(pk)), v),
            _ => table_set_existing_cold(tb, *pk, v),
        }
    }
}

/// Write `tb[*pk] := v` when `*pk` is an integer in the array part, as
/// a raw set does; `false`, having done nothing, otherwise.
///
/// # Safety
/// `pk` points at an initialised value.
#[inline(always)]
unsafe fn table_set_array_at(tb: &mut Table, pk: *const Value, v: Value) -> bool {
    // SAFETY: the caller's contract; the payload is read after the tag
    unsafe {
        if raw_tag(pk) != tag::INT {
            return false;
        }
        let i = raw_int(pk);
        if i < 1 || i as u64 > tb.asize {
            return false;
        }
        tb.aset(i as usize - 1, v);
    }
    true
}

#[cold]
#[inline(never)]
fn table_set_existing_cold(tb: &mut Table, key: Value, v: Value) -> bool {
    tb.try_set_existing(key, v)
}

impl Vm {
    /// [`Self::index_raw`] with the table and the key read in place, which
    /// keeps them out of stack slots in the fast loop.
    ///
    /// # Safety
    /// `pt` and `pk` point at initialised values.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn index_raw_at(pt: *const Value, pk: *const Value) -> Option<Value> {
        // gc-verify builds keep every read on the probed path
        #[cfg(not(feature = "gc-verify"))]
        // SAFETY: the caller's contract; a table tag means a live table
        unsafe {
            if raw_tag(pt) == tag::TABLE {
                let tb = &*(raw_gc(pt) as *const Table);
                let v = table_get_at(tb, pk);
                if !v.is_nil() || tb.metatable().is_none() {
                    return Some(v);
                }
            }
        }
        None
    }

    /// [`Self::index_raw`] on a table value with the key read in place.
    ///
    /// # Safety
    /// `pk` points at an initialised value.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn index_raw_key_at(t: Value, pk: *const Value) -> Option<Value> {
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t {
            // SAFETY: the caller's contract
            let v = unsafe { table_get_at(&tb, pk) };
            if !v.is_nil() || tb.metatable().is_none() {
                return Some(v);
            }
        }
        None
    }

    /// [`Self::newindex_raw`] with the table and the key read in place.
    ///
    /// # Safety
    /// `pt` and `pk` point at initialised values.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn newindex_raw_at(
        &mut self,
        pt: *const Value,
        pk: *const Value,
        v: Value,
    ) -> bool {
        #[cfg(not(feature = "gc-verify"))]
        // SAFETY: the caller's contract; a table tag means a live table,
        // which nothing else borrows while an opcode runs
        unsafe {
            if raw_tag(pt) == tag::TABLE {
                let tb = raw_gc(pt) as *mut Table;
                if table_set_existing_at(&mut *tb, pk, v)
                    || (*tb).metatable().is_none() && table_set_array_at(&mut *tb, pk, v)
                {
                    self.heap
                        .barrier_back(tb as *mut crate::runtime::heap::GcHeader);
                    return true;
                }
            }
        }
        false
    }

    /// [`Self::newindex_raw_at`] on a table value.
    ///
    /// # Safety
    /// `pk` points at an initialised value.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn newindex_raw_key_at(
        &mut self,
        t: Value,
        pk: *const Value,
        v: Value,
    ) -> bool {
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t
            // SAFETY: the caller's contract; see `newindex_raw`
            && unsafe { table_set_existing_at(tb.as_mut(), pk, v) }
        {
            self.heap
                .barrier_back(tb.as_ptr() as *mut crate::runtime::heap::GcHeader);
            return true;
        }
        false
    }

    /// `t[key]` without `__index` (PUC `luaV_fastget`): a raw hit on a table
    /// is the result, since `__index` is consulted only when the raw value is
    /// nil, and a miss on a table without a metatable is nil. `None` leaves
    /// the read to [`Self::index_miss`].
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) fn index_raw(&self, t: Value, key: Value) -> Option<Value> {
        // gc-verify builds keep every read on the probed path
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t {
            let v = match key {
                Value::Str(s) => tb.get_str(s),
                Value::Int(i) => tb.get_int(i),
                k => tb.get(k),
            };
            if !v.is_nil() || tb.metatable().is_none() {
                return Some(v);
            }
        }
        None
    }

    /// A read opcode that [`Self::index_raw`] could not finish: continues the
    /// `__index` chain without repeating the raw probe.
    #[inline(never)]
    pub(super) fn index_miss(&mut self, t: Value, key: Value, dst: u32) -> Result<(), LuaError> {
        #[cfg(not(feature = "gc-verify"))]
        {
            if let Value::Str(s) = key {
                return self.index_str_miss(t, s, dst);
            }
            self.op_index_from(t, key, dst, matches!(t, Value::Table(_)))
        }
        #[cfg(feature = "gc-verify")]
        self.op_index(t, key, dst)
    }

    /// `t[key] := v` without `__newindex` (PUC `luaV_fastset`): overwriting a
    /// key that is present with a non-nil value never involves `__newindex`.
    /// `false` leaves the write to [`Self::newindex_miss`], having done nothing.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) fn newindex_raw(&mut self, t: Value, key: Value, v: Value) -> bool {
        // gc-verify builds keep every write on the probed path
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            && unsafe { tb.as_mut() }.try_set_existing(key, v)
        {
            self.heap
                .barrier_back(tb.as_ptr() as *mut crate::runtime::heap::GcHeader);
            return true;
        }
        false
    }

    /// A write opcode that [`Self::newindex_raw`] could not finish: runs the
    /// `__newindex` chain without repeating that probe.
    #[inline(never)]
    pub(super) fn newindex_miss(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        // with no `__newindex` to call the write is a raw set (PUC
        // `luaV_finishset`)
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t
            && tb
                .metatable()
                .is_none_or(|mt| self.fast_tm(mt, Mm::NewIndex).is_nil())
        {
            return self.raw_set(tb, key, v);
        }
        let probed = cfg!(not(feature = "gc-verify")) && matches!(t, Value::Table(_));
        self.op_newindex_from(t, key, v, probed)
    }
}
