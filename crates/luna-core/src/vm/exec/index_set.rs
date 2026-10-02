//! The in-place table writes the dispatch loop finishes itself.

// gc-verify builds take every write through the probed paths
#![cfg_attr(feature = "gc-verify", allow(dead_code))]

use super::*;
use crate::runtime::string::LuaStr;
use crate::runtime::value::tag;
use fast_arith::{raw_gc, raw_int, raw_tag};

/// Overwrite `tb[*pk]` with `*pv` when the key is present with a non-nil
/// value (PUC `luaV_fastset`); `false`, having done nothing, otherwise.
/// The value is copied from its register in place (see [`Value::copy_raw`]).
///
/// # Safety
/// `pk` and `pv` point at initialised values.
#[inline(always)]
pub(super) unsafe fn table_set_existing_at(
    tb: &mut Table,
    pk: *const Value,
    pv: *const Value,
) -> bool {
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
pub(super) unsafe fn table_raw_set_at(
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
