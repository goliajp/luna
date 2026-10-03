//! Table read and length helpers.

use crate::{current_jit_closure, current_jit_vm, payload_bits, raw_bits, str_arg, table_arg};

/// Read `t[key_ptr_as_str]` and return raw payload bits.
/// String key is a `Gc<LuaStr>` raw pointer baked into IR. Caller
/// (trace JIT GetField emit) infers exit_tag for the dst slot via
/// `infer_getx_exit`; absent inference, dispatchable=false.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table and
/// `key_ptr` an interned string.
// SAFETY: no other item in the link is named `luna_jit_table_get_field`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_get_field(t: i64, key_ptr: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return 0;
    }
    // SAFETY: `key_ptr` is an interned string (# Safety)
    let key_gc = unsafe { str_arg(key_ptr) };
    payload_bits(g.get_str(key_gc))
}

/// Read `upvals[upval_idx][key_str]` and return raw
/// payload bits. Mirrors `luna_jit_table_get_field` but resolves the
/// table via the trace head closure's upvalue list first (the trace
/// dispatcher's `enter_jit(vm, Some(cl))` pins `JIT_CL`).
///
/// Used by the trace JIT lowerer's `Op::GetTabUp` arm for upvalue-
/// table accesses outside the recognised math-fold pattern. The
/// canonical case is `math.min(a, b)` whose 2-arg shape doesn't
/// match `try_match_trace_math_fold`'s single-arg libm catalog;
/// without this helper the entire trace bails at the `cmp-dirs`
/// pre-emit pass and the workload runs interp-only (e.g.
/// `bail:cmp-dirs-GetTabUp` × 200/200 on `token_bucket_1k`).
///
/// Deopt cases: upval isn't a Table (corrupted upval list) or has
/// a metatable (`__index` could shadow the lookup — interp-only).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread opened with the running
/// closure; `key_ptr` is an interned string.
// SAFETY: no other item in the link is named `luna_jit_op_get_tab_up`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_get_tab_up(upval_idx: i64, key_ptr: i64) -> i64 {
    // SAFETY: inside an enter_jit window opened with the running closure (# Safety) JIT_VM is the
    // Vm lent to this call and JIT_CL that closure
    let (vm, cl) = unsafe { (current_jit_vm(), current_jit_closure()) };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let env = vm.upval_get(cl, upval_idx as u32);
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> = match env {
        luna_core::runtime::Value::Table(t) => t,
        _ => {
            vm.jit.pending_err = Some(vm.rt_err("JIT deopt: GetTabUp upval not Table"));
            return 0;
        }
    };
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: GetTabUp env has metatable"));
        return 0;
    }
    // SAFETY: `key_ptr` is an interned string (# Safety)
    let key_gc = unsafe { str_arg(key_ptr) };
    payload_bits(g.get_str(key_gc))
}

/// Trace-JIT table reads that check what they read. The trace types a
/// read's result from how the next ops use it (arithmetic → Int, indexing
/// → Table, ...) and compiles the rest of the trace for that type. These
/// variants return `1` and write the payload through `out` only when the
/// value has tag `want_tag` and the table has no metatable (whose
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
    let (tag, raw) = v.unpack();
    if tag as i64 != want_tag {
        return 0;
    }
    // SAFETY: `out` is writable, by the caller's contract
    unsafe { *out = raw_bits(raw) };
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

/// `t[key]` where the JIT statically expects an Int
/// result. Pulls the raw `Value` from the table and unpacks
/// the Int payload. If the slot is anything but Int (Nil, Float,
/// Str, …) the helper returns 0 — the JIT scan only admits
/// chunks that store Ints, so the divergence is observable only
/// when the user-facing semantics violate the static expectation.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table.
// SAFETY: no other item in the link is named `luna_jit_table_get_int`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_get_int(t: i64, key: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    // Metatable on the source table means PUC would route
    // a missing entry through __index; the helper bypasses that. Park a
    // deopt request and bail; the dispatcher re-runs the call through
    // the interpreter, which walks __index correctly (including the
    // infinite-loop error events.lua relies on).
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return 0;
    }
    // Return the raw 8-byte payload of the stored
    // Value, regardless of tag. The JIT-emitted caller interprets
    // the bit pattern according to the GetI result's RegKind:
    // Int → i64, Float → f64::from_bits, Table → Gc<Table>::from_ptr.
    // A previous variant unconditionally returned 0 on non-Int /
    // non-Float — that fed NULL into subsequent helpers when the
    // table actually stored Gc objects (binary_trees' `check`
    // chain calling itself on `t[1]`).
    payload_bits(g.get_int(key))
}

/// `t[k]` where `k` is a Float key. luna 5.1 / 5.2's
/// `OP_GETTABLE` typically loads the key via `LoadF` (no Int subtype
/// in those dialects); the emit hands `k` as `f64::to_bits` so the
/// helper can reconstruct the Float value before calling `Table::get`.
/// `Table::get` normalises integral Floats back to the Int slot, so
/// `t[1.0]` lands on `t[1]` exactly like PUC does. Returns the raw
/// 8-byte payload (same convention as `luna_jit_table_get_int`).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table.
// SAFETY: no other item in the link is named `luna_jit_table_get_float`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_get_float(t: i64, key_bits: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return 0;
    }
    let k = luna_core::runtime::Value::Float(f64::from_bits(key_bits as u64));
    payload_bits(g.get(k))
}

/// `#t` (table length).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table.
// SAFETY: no other item in the link is named `luna_jit_table_len`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_len(t: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    // 5.4+ honours __len on tables; the helper bypasses it.
    // Park a deopt request and let the interpreter compute the length.
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return 0;
    }
    g.len()
}

/// The trace JIT's `#t`: the length, or `-1` when the table has a
/// metatable (whose `__len` the helper would bypass), in which case the
/// caller side-exits at the op and the interpreter performs it.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table.
// SAFETY: no other item in the link is named `luna_jit_table_len_checked`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_len_checked(t: i64) -> i64 {
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent
        // to this call
        unsafe { current_jit_vm() }.jit.counters.deopt += 1;
        return -1;
    }
    g.len()
}
