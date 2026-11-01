//! The trace JIT's table stores, which leave a store the interpreter must
//! make (a metatable, a read-only table, a bad key) to it.

use crate::table_write::barrier_for;
use crate::{current_jit_vm, str_arg, table_arg};
use luna_core::__private::VmHeap;
use luna_core::__private::VmJitState;
use luna_core::__private::{GcInternals, TableInternals, ValueInternals};

/// The trace JIT's table stores, `t[key] = val`, with the value given as
/// a `raw` tag and payload. They return `1` when stored. They store
/// nothing and return `0` when the table has a metatable, whose
/// `__newindex` the helper would bypass, when it is read-only, or when the
/// key cannot index a
/// table (nil, NaN); the caller then side-exits at the storing op and
/// the interpreter performs it. The trace goes no further than that op,
/// so nothing it did before is repeated.
///
/// # Safety
/// Called inside an `enter_jit` window on this thread; `t` is a live
/// table, and `val_tag` is the tag of a value whose payload is `val_raw`
/// (see `runtime::value::raw`).
#[inline]
unsafe fn checked_store(t: i64, key: luna_core::runtime::Value, val_raw: i64, val_tag: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to
    // this call
    let vm = unsafe { current_jit_vm() };
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    // a read-only table is left to the interpreter too, which raises
    if g.metatable().is_some() || g.is_readonly() {
        vm.jit_counters_mut().deopt += 1;
        return 0;
    }
    // SAFETY: `t` is a live table and (`val_tag`, `val_raw`) one value's
    // tag and payload (# Safety); `g` is not dereferenced again while the
    // borrow is in use, and `Table::set` never runs the collector; the table
    // is not read-only (tested above), the barrier is taken below, and the
    // heap stays in place
    let (val, r) = unsafe {
        let raw = luna_core::__private::RawVal {
            zero: val_raw as u64,
        };
        let val = luna_core::runtime::Value::pack(val_tag as u8, raw);
        (val, g.as_mut().set_unguarded(vm.heap_mut(), key, val))
    };
    if r.is_err() {
        vm.jit_counters_mut().deopt += 1;
        return 0;
    }
    // SAFETY: `g` is a live table of the Vm lent to this call and `val` one of its values (# Safety)
    unsafe { barrier_for(vm, g, key, val) };
    1
}

/// `t[key] = val` with an integer key; see `checked_store`.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table of
/// the Vm the window lends, its values that Vm's, and `val_tag` and `val_raw` are one value's tag
/// and payload.
// SAFETY: no other item in the link is named `luna_jit_table_set_int_checked`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_int_checked(
    t: i64,
    key: i64,
    val_raw: i64,
    val_tag: i64,
) -> i64 {
    // SAFETY: `checked_store` asks what this helper's own contract
    // guarantees (# Safety)
    unsafe { checked_store(t, luna_core::runtime::Value::Int(key), val_raw, val_tag) }
}

/// `t[key] = val` with an interned string key; see `checked_store`.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table of
/// the Vm the window lends, its values that Vm's, `key_ptr` an interned string, and `val_tag` and
/// `val_raw` one value's tag and payload.
// SAFETY: no other item in the link is named `luna_jit_table_set_field_checked`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_field_checked(
    t: i64,
    key_ptr: i64,
    val_raw: i64,
    val_tag: i64,
) -> i64 {
    // SAFETY: `key_ptr` is an interned string (# Safety)
    let key = unsafe { str_arg(key_ptr) };
    // SAFETY: `checked_store` asks what this helper's own contract
    // guarantees (# Safety)
    unsafe { checked_store(t, luna_core::runtime::Value::Str(key), val_raw, val_tag) }
}

/// `t[key] = val` with a key of any type, given like the value; see
/// `checked_store`.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table of
/// the Vm the window lends, its values that Vm's, and each of (`key_tag`, `key_raw`) and
/// (`val_tag`, `val_raw`) is one value's tag and payload.
// SAFETY: no other item in the link is named `luna_jit_table_set_checked`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_checked(
    t: i64,
    key_raw: i64,
    key_tag: i64,
    val_raw: i64,
    val_tag: i64,
) -> i64 {
    // SAFETY: (`key_tag`, `key_raw`) is one value's tag and payload, and
    // the rest is what `checked_store` asks (# Safety)
    unsafe {
        let raw = luna_core::__private::RawVal {
            zero: key_raw as u64,
        };
        let key = luna_core::runtime::Value::pack(key_tag as u8, raw);
        checked_store(t, key, val_raw, val_tag)
    }
}
