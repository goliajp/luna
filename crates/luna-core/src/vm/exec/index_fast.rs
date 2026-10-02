//! The table reads and writes the dispatch loop finishes itself, and the
//! hand-over to the metamethod chains for the rest.

// gc-verify builds take every read and write through the probed paths
#![cfg_attr(feature = "gc-verify", allow(dead_code))]

use super::*;
use crate::runtime::string::LuaStr;
use crate::runtime::value::tag;
use fast_arith::{raw_gc, raw_int, raw_tag};

/// Raw `tb[*pk]` with the key read in place (strings by pointer, integers
/// in the array part, other keys by the general lookup out of line),
/// copied to `dst` when it settles the read without `__index`: a non-nil
/// value, or any value of a table without a metatable. `false`, with `dst`
/// untouched, leaves the read to the `__index` chain. The value goes from
/// its slot to `dst` directly, as PUC `setobj2s` does: passed back as a
/// `Value` it took a detour through stack slots.
///
/// # Safety
/// `pk` points at an initialised value and `dst` at a register; `dst` is
/// written only after `pk` is read.
#[inline(always)]
unsafe fn table_get_into(tb: &Table, pk: *const Value, dst: *mut Value) -> bool {
    let plain = tb.metatable().is_none();
    // SAFETY: the caller's contract; payloads are read after their tags
    unsafe {
        match raw_tag(pk) {
            tag::STR => {
                let key = Gc::from_ptr_unchecked(raw_gc(pk) as *mut LuaStr);
                match tb.str_slot_by_ptr(key) {
                    Some(slot) => {
                        if !plain && raw_tag(slot) == tag::NIL {
                            return false;
                        }
                        Value::copy_raw(dst, slot);
                        true
                    }
                    None if key.is_short() => {
                        if plain {
                            dst.write(Value::Nil);
                        }
                        plain
                    }
                    None => table_get_cold(tb, *pk, dst, plain),
                }
            }
            tag::INT => {
                let i = raw_int(pk);
                if i >= 1 && i as u64 <= tb.asize {
                    let idx = i as usize - 1;
                    let t = *tb.atags().get_unchecked(idx);
                    if !plain && t == crate::runtime::value::raw::NIL {
                        return false;
                    }
                    Value::pack_into(dst, t, *tb.avals().get_unchecked(idx));
                    return true;
                }
                table_get_cold(tb, *pk, dst, plain)
            }
            _ => table_get_cold(tb, *pk, dst, plain),
        }
    }
}

/// [`table_get_into`] for a string constant key (`GetField`, `GetTabUp`, a
/// `k` `SelfOp`): the compiler, the PUC translators and the bytecode
/// verifier keep those keys strings, so the tag is not looked at.
///
/// # Safety
/// As for `table_get_into`, and `pk` points at a string.
#[inline(always)]
unsafe fn table_get_kstr_into(tb: &Table, pk: *const Value, dst: *mut Value) -> bool {
    // SAFETY: the caller's contract
    unsafe {
        debug_assert_eq!(raw_tag(pk), tag::STR);
        let key = Gc::from_ptr_unchecked(raw_gc(pk) as *mut LuaStr);
        match tb.str_slot_by_ptr(key) {
            Some(slot) => {
                if raw_tag(slot) == tag::NIL && tb.metatable().is_some() {
                    return false;
                }
                Value::copy_raw(dst, slot);
                true
            }
            None if key.is_short() => {
                let plain = tb.metatable().is_none();
                if plain {
                    dst.write(Value::Nil);
                }
                plain
            }
            None => table_get_cold(tb, *pk, dst, tb.metatable().is_none()),
        }
    }
}

/// [`table_get_into`] for the keys it does not look up inline.
///
/// # Safety
/// As for `table_get_into`.
#[cold]
#[inline(never)]
unsafe fn table_get_cold(tb: &Table, key: Value, dst: *mut Value, plain: bool) -> bool {
    let v = tb.get(key);
    if v.is_nil() && !plain {
        return false;
    }
    // SAFETY: the caller's contract
    unsafe { dst.write(v) };
    true
}

/// Overwrite `tb[*pk]` with `*pv` when the key is present with a non-nil
/// value (PUC `luaV_fastset`); `false`, having done nothing, otherwise.
/// The value is copied from its register in place (see [`Value::copy_raw`]).
///
/// # Safety
/// `pk` and `pv` point at initialised values.
#[inline(always)]
unsafe fn table_set_existing_at(tb: &mut Table, pk: *const Value, pv: *const Value) -> bool {
    // SAFETY: the caller's contract; payloads are read after their tags
    unsafe {
        match raw_tag(pk) {
            tag::STR => {
                let key = Gc::from_ptr_unchecked(raw_gc(pk) as *mut LuaStr);
                match tb.str_slot_by_ptr_mut(key) {
                    // a nil value in a node is how a removed key is kept
                    Some(slot) if raw_tag(slot) != tag::NIL => {
                        Value::copy_raw(slot, pv);
                        true
                    }
                    Some(_) => false,
                    None if key.is_short() => false,
                    None => table_set_existing_cold(tb, *pk, *pv),
                }
            }
            tag::INT => {
                let i = raw_int(pk);
                if i >= 1 && i as u64 <= tb.asize {
                    let idx = i as usize - 1;
                    if *tb.atags().get_unchecked(idx) == crate::runtime::value::raw::NIL {
                        return false;
                    }
                    tb.aset_at(idx, pv);
                    return true;
                }
                table_set_existing_cold(tb, Value::Int(i), *pv)
            }
            _ => table_set_existing_cold(tb, *pk, *pv),
        }
    }
}

/// `tb[*pk] := *pv` as a raw set (PUC `luaH_finishset` with no `__newindex`
/// to call): an integer in the array part in place, anything else through
/// [`Table::set`] out of line. `false` when that refuses the key (nil, NaN,
/// overflow), having done nothing; the miss path then raises the error.
///
/// # Safety
/// `pk` and `pv` point at initialised values.
#[inline(always)]
unsafe fn table_raw_set_at(
    tb: &mut Table,
    heap: &mut Heap,
    pk: *const Value,
    pv: *const Value,
) -> bool {
    // SAFETY: the caller's contract; the payload is read after the tag
    unsafe {
        if raw_tag(pk) == tag::INT {
            let i = raw_int(pk);
            if i >= 1 && i as u64 <= tb.asize {
                tb.aset_at(i as usize - 1, pv);
                return true;
            }
        }
        table_raw_set_cold(tb, heap, *pk, *pv)
    }
}

#[cold]
#[inline(never)]
fn table_raw_set_cold(tb: &mut Table, heap: &mut Heap, key: Value, v: Value) -> bool {
    tb.set(heap, key, v).is_ok()
}

#[cold]
#[inline(never)]
fn table_set_existing_cold(tb: &mut Table, key: Value, v: Value) -> bool {
    tb.try_set_existing(key, v)
}

impl Vm {
    /// `t[key]` without `__index` (PUC `luaV_fastget`), with the table and the key read in place and
    /// the result copied to `dst` (see [`table_get_into`]); `false`, with
    /// `dst` untouched, leaves the read to [`Self::index_miss`].
    ///
    /// # Safety
    /// `pt` and `pk` point at initialised values and `dst` at a register.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn index_raw_at(pt: *const Value, pk: *const Value, dst: *mut Value) -> bool {
        // gc-verify builds keep every read on the probed path
        #[cfg(not(feature = "gc-verify"))]
        // SAFETY: the caller's contract; a table tag means a live table
        unsafe {
            if raw_tag(pt) == tag::TABLE {
                return table_get_into(&*(raw_gc(pt) as *const Table), pk, dst);
            }
        }
        false
    }

    /// [`Self::index_raw_at`] for a string constant key (see
    /// [`table_get_kstr_into`]).
    ///
    /// # Safety
    /// As for `index_raw_at`, and `pk` points at a string.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn index_raw_kstr_at(
        pt: *const Value,
        pk: *const Value,
        dst: *mut Value,
    ) -> bool {
        #[cfg(not(feature = "gc-verify"))]
        // SAFETY: the caller's contract; a table tag means a live table
        unsafe {
            if raw_tag(pt) == tag::TABLE {
                return table_get_kstr_into(&*(raw_gc(pt) as *const Table), pk, dst);
            }
        }
        false
    }

    /// [`Self::index_raw_kstr_at`] on a table value.
    ///
    /// # Safety
    /// `pk` points at a string and `dst` at a register.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn index_raw_kstr_key_at(
        t: Value,
        pk: *const Value,
        dst: *mut Value,
    ) -> bool {
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t {
            // SAFETY: the caller's contract
            return unsafe { table_get_kstr_into(&tb, pk, dst) };
        }
        false
    }

    /// `t[key] := v` without `__newindex` (PUC `luaV_fastset`), with the
    /// table, the key and the value read in place: overwriting a key that is
    /// present with a non-nil value never involves `__newindex`, and with no
    /// metatable the write is a raw set. `false` leaves the write to
    /// [`Self::newindex_miss`], having done nothing.
    ///
    /// # Safety
    /// `pt`, `pk` and `pv` point at initialised values.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn newindex_raw_at(
        &mut self,
        pt: *const Value,
        pk: *const Value,
        pv: *const Value,
    ) -> bool {
        #[cfg(not(feature = "gc-verify"))]
        // SAFETY: the caller's contract; a table tag means a live table,
        // which nothing else borrows while an opcode runs
        unsafe {
            if raw_tag(pt) == tag::TABLE {
                let tb = raw_gc(pt) as *mut Table;
                if table_set_existing_at(&mut *tb, pk, pv)
                    || (*tb).metatable().is_none()
                        && table_raw_set_at(&mut *tb, &mut self.heap, pk, pv)
                {
                    self.heap
                        .barrier_back(tb as *mut crate::runtime::heap::GcHeader);
                    return true;
                }
            }
        }
        false
    }

    /// [`Self::newindex_raw_at`] for a string constant key (`SetField`; see
    /// [`table_get_kstr_into`]): an existing key is overwritten here, the
    /// rest goes through [`Self::newindex_raw_at`] out of line.
    ///
    /// # Safety
    /// As for `newindex_raw_at`, and `pk` points at a string.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn newindex_raw_kstr_at(
        &mut self,
        pt: *const Value,
        pk: *const Value,
        pv: *const Value,
    ) -> bool {
        #[cfg(not(feature = "gc-verify"))]
        // SAFETY: the caller's contract; a table tag means a live table
        unsafe {
            debug_assert_eq!(raw_tag(pk), tag::STR);
            if raw_tag(pt) == tag::TABLE {
                let tb = raw_gc(pt) as *mut Table;
                let key = Gc::from_ptr_unchecked(raw_gc(pk) as *mut LuaStr);
                // a nil value in a node is how a removed key is kept
                if let Some(slot) = (*tb).str_slot_by_ptr_mut(key)
                    && raw_tag(slot) != tag::NIL
                {
                    Value::copy_raw(slot, pv);
                    self.heap
                        .barrier_back(tb as *mut crate::runtime::heap::GcHeader);
                    return true;
                }
            }
        }
        self.newindex_raw_at_cold(pt, pk, pv)
    }

    #[cold]
    #[inline(never)]
    fn newindex_raw_at_cold(
        &mut self,
        pt: *const Value,
        pk: *const Value,
        pv: *const Value,
    ) -> bool {
        // SAFETY: as for `newindex_raw_kstr_at`
        unsafe { self.newindex_raw_at(pt, pk, pv) }
    }

    /// [`Self::newindex_raw_at`] on a table value.
    ///
    /// # Safety
    /// `pk` and `pv` point at initialised values.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) unsafe fn newindex_raw_key_at(
        &mut self,
        t: Value,
        pk: *const Value,
        pv: *const Value,
    ) -> bool {
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t
            // SAFETY: the caller's contract; see `newindex_raw_at`
            && unsafe { table_set_existing_at(tb.as_mut(), pk, pv) }
        {
            self.heap
                .barrier_back(tb.as_ptr() as *mut crate::runtime::heap::GcHeader);
            return true;
        }
        false
    }
}
