//! Table reads that check the type of what they read.

use crate::{current_jit_closure, current_jit_vm, raw_bits, str_arg, table_arg};
use luna_core::__private::{ValueInternals, VmHostInternals};

/// Trace-JIT table reads that check what they read. The trace types a
/// read's result from how the next ops use it (arithmetic → Int, indexing
/// → Table, ...) and compiles the rest of the trace for that type. These
/// variants return `1` and write the payload through `out` only when the
/// value has tag `want_tag` (`raw::FALSE`: a boolean of either value,
/// written as 0 or 1) and the table has no metatable (whose
/// `__index` the helper would bypass); otherwise they return `0` and the
/// caller side-exits at the reading op, so the interpreter performs it.
///
/// # Safety
/// `out` is valid for writing one `i64`.
pub(crate) unsafe fn checked_read(
    v: luna_core::runtime::Value,
    want_tag: i64,
    out: *mut i64,
) -> i64 {
    use luna_core::__private::raw;
    let (tag, payload) = v.unpack();
    // `raw::FALSE` asks for a boolean of either value, as 0 or 1
    let bits = match tag {
        raw::FALSE | raw::TRUE if want_tag == i64::from(raw::FALSE) => i64::from(tag - raw::FALSE),
        _ if tag as i64 == want_tag => raw_bits(payload),
        _ => return 0,
    };
    // SAFETY: `out` is writable, by the caller's contract
    unsafe { *out = bits };
    1
}

/// `t[key]` with an integer key; see `checked_read`.
///
/// # Safety
/// `t` is a live table and `out` is valid for writing one `i64`.
// SAFETY: no other item in the link is named `luna_jit_table_get_int_checked`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_get_int_checked(
    t: i64,
    key: i64,
    want_tag: i64,
    out: *mut i64,
) -> i64 {
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        return 0;
    }
    // SAFETY: `out` is writable (# Safety)
    unsafe { checked_read(g.get_int(key), want_tag, out) }
}

/// `t[key]` with a float key (its bits); see `checked_read`. The table
/// normalises an integral key and finds nothing for a NaN.
///
/// # Safety
/// `t` is a live table and `out` is valid for writing one `i64`.
// SAFETY: no other item in the link is named `luna_jit_table_get_float_checked`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_get_float_checked(
    t: i64,
    key_bits: i64,
    want_tag: i64,
    out: *mut i64,
) -> i64 {
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        return 0;
    }
    let k = luna_core::runtime::Value::Float(f64::from_bits(key_bits as u64));
    // SAFETY: `out` is writable (# Safety)
    unsafe { checked_read(g.get(k), want_tag, out) }
}

/// `t[key]` with an interned string key; see `checked_read`.
///
/// # Safety
/// `t` is a live table, `key_ptr` an interned string, and `out` is valid for writing one `i64`.
// SAFETY: no other item in the link is named `luna_jit_table_get_field_checked`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_get_field_checked(
    t: i64,
    key_ptr: i64,
    want_tag: i64,
    out: *mut i64,
) -> i64 {
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        return 0;
    }
    // SAFETY: `key_ptr` is an interned string (# Safety)
    let key = unsafe { str_arg(key_ptr) };
    // SAFETY: `out` is writable (# Safety)
    unsafe { checked_read(g.get_str(key), want_tag, out) }
}

/// `upvals[upval_idx][key]` (a global read through `_ENV`); see
/// `checked_read`. An upvalue that is not a plain table also fails.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread opened with the running
/// closure; `key_ptr` is an interned string and `out` is valid for writing one `i64`.
// SAFETY: no other item in the link is named `luna_jit_op_get_tab_up_checked`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_get_tab_up_checked(
    upval_idx: i64,
    key_ptr: i64,
    want_tag: i64,
    out: *mut i64,
) -> i64 {
    // SAFETY: inside an enter_jit window opened with the running closure (# Safety) JIT_VM is the
    // Vm lent to this call and JIT_CL that closure
    let (vm, cl) = unsafe { (current_jit_vm(), current_jit_closure()) };
    let luna_core::runtime::Value::Table(g) = vm.upval_get(cl, upval_idx as u32) else {
        return 0;
    };
    if g.metatable().is_some() {
        return 0;
    }
    // SAFETY: `key_ptr` is an interned string (# Safety)
    let key = unsafe { str_arg(key_ptr) };
    // SAFETY: `out` is writable (# Safety)
    unsafe { checked_read(g.get_str(key), want_tag, out) }
}
