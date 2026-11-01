//! Table store helpers.

use crate::{current_jit_vm, str_arg, table_arg};
use luna_core::__private::VmJitState;
use luna_core::__private::{GcInternals, TableInternals, ValueInternals, VmJitAlloc};
use luna_core::__private::{HeapBarriers, TableStores, VmHeap};

/// `t[key] = val` where `t` is a Table Gc (i64 pun), `key`
/// is an Int and `val` is an Int. Wraps
/// `TableStores::store_int`. Returns 1 when stored, 0 when it parked a deopt (a
/// metatable, or a table that could not grow); the method JIT's code
/// then returns at once and the interpreter runs the call.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table of
/// the Vm the window lends, its values that Vm's.
// SAFETY: no other item in the link is named `luna_jit_table_set_int`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_int(t: i64, key: i64, val: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit().pending_err.is_some() {
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    // A metatable on the target table means PUC would route
    // this write through __newindex; the JIT helper would bypass it. Park
    // a deopt request and let the dispatcher re-run the call through the
    // interpreter so __newindex / raw-set semantics are honoured.
    if g.metatable().is_some() {
        let e = vm.runtime_error("JIT deopt: table has metatable");
        vm.set_jit_pending_err(e);
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety); `g` is not dereferenced again
    // while the borrow is in use, and `Table::set*` never runs the
    // collector; the method JIT stores only into tables its call tested on
    // entry or made itself (see `Vm::try_jit_call_op`), an integer needs
    // no barrier, and the heap stays in place
    let r = unsafe {
        g.as_mut()
            .set_int_unguarded(vm.heap_mut(), key, luna_core::runtime::Value::Int(val))
    };
    stored(vm, r)
}

/// Write an arbitrary `Value::pack(tag, raw_bits)` to
/// `t[key]` (Int key). Generalises `_table_set_int` / `_table_set_nil`:
/// trace JIT emit dispatches Int/Nil to their specialized helpers
/// (slightly less overhead) and Closure/Table/Float/etc. to this
/// helper. Without it, a SetTable whose src is a Closure (from
/// Op::Closure trace JIT) silently wraps the closure pointer as
/// `Value::Int(ptr_bits)` — a number that later calls fail with
/// "attempt to call a number value".
///
/// No compiler emits this call any more: the trace JIT stores through
/// the `luna_jit_table_set_*_checked` helpers. It stays for the 3.x API.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table of
/// the Vm the window lends, its values that Vm's, and `tag` and `raw_bits` are one value's tag and
/// payload.
// SAFETY: no other item in the link is named `luna_jit_table_set_raw`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_raw(t: i64, key: i64, raw_bits: i64, tag: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit().pending_err.is_some() {
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        let e = vm.runtime_error("JIT deopt: table has metatable");
        vm.set_jit_pending_err(e);
        return 0;
    }
    // SAFETY: (`tag`, `raw_bits`) is one value's tag and payload (# Safety)
    let v = unsafe {
        let raw = luna_core::__private::RawVal {
            zero: raw_bits as u64,
        };
        luna_core::runtime::Value::pack(tag as u8, raw)
    };
    // SAFETY: `g` is a live table of the Vm lent to this call (# Safety) and `v` one of its values;
    // no reference to the table is held across the store, and the heap stays in place
    let r = unsafe { g.store_int(vm.heap_mut(), key, v) };
    if refused(vm, r) {
        return 0;
    }
    1
}

/// Write `Value::pack(tag, raw)` to `t[key_ptr_as_str]`.
/// String key is a `Gc<LuaStr>` raw pointer (baked into IR at
/// emit time from `head_proto.consts[ins.b()]`); value goes
/// through the standard tag/raw round-trip. Used for Op::SetField
/// trace JIT support (helper path; the sunk emit path is separate).
///
/// Same metatable / pending_err short-circuit as the other table
/// helpers — `__newindex` cases deopt to interp.
///
/// No compiler emits this call any more: the trace JIT stores through
/// the `luna_jit_table_set_*_checked` helpers. It stays for the 3.x API.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table of
/// the Vm the window lends, its values that Vm's, `key_ptr` an interned string, and `val_tag` and
/// `val_raw` one value's tag and payload.
// SAFETY: no other item in the link is named `luna_jit_table_set_field`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_field(
    t: i64,
    key_ptr: i64,
    val_raw: i64,
    val_tag: i64,
) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit().pending_err.is_some() {
        return;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        let e = vm.runtime_error("JIT deopt: table has metatable");
        vm.set_jit_pending_err(e);
        return;
    }
    // SAFETY: `key_ptr` is an interned string (# Safety)
    let key_gc = unsafe { str_arg(key_ptr) };
    let key = luna_core::runtime::Value::Str(key_gc);
    // SAFETY: (`val_tag`, `val_raw`) is one value's tag and payload
    // (# Safety)
    let v = unsafe {
        let raw = luna_core::__private::RawVal {
            zero: val_raw as u64,
        };
        luna_core::runtime::Value::pack(val_tag as u8, raw)
    };
    // a refusal parks the deopt request, the store's only outcome here
    // SAFETY: `g` is a live table of the Vm lent to this call (# Safety), `key` and `v` its values;
    // no reference to the table is held across the store, and the heap stays in place
    let r = unsafe { g.store(vm.heap_mut(), key, v) };
    refused(vm, r);
}

/// Whether `r`, a store's result, is a read-only table's refusal or a
/// table that could not grow; if so, park a deopt request, as for a table
/// with a metatable, so that the interpreter runs the call again and
/// raises the error or makes the table grow. For the method JIT's stores
/// and the helpers no compiler emits any more, which any caller may hand
/// any table.
fn refused(vm: &mut luna_core::vm::Vm, r: Result<(), luna_core::runtime::TableError>) -> bool {
    match r {
        Err(luna_core::runtime::TableError::ReadOnly) => {
            let e = vm.runtime_error("JIT deopt: table is read-only");
            vm.set_jit_pending_err(e);
            true
        }
        Err(luna_core::runtime::TableError::Memory(_)) => {
            vm.jit_park_oom();
            true
        }
        _ => false,
    }
}

/// [`refused`] as the status the method JIT's stores return: 1 stored,
/// 0 parked.
fn stored(vm: &mut luna_core::vm::Vm, r: Result<(), luna_core::runtime::TableError>) -> i64 {
    i64::from(!refused(vm, r))
}

/// The write barrier for a store of `key` / `val` into `g`, as the
/// interpreter's stores take it: a white object in a table the collector
/// already traced (black) sends the table back to be traced again, or the
/// new object would be swept while the table holds it.
///
/// # Safety
/// `g` is a live table of `vm`'s heap, and `key` and `val` are `vm`'s values.
#[inline]
pub(crate) unsafe fn barrier_for(
    vm: &mut luna_core::vm::Vm,
    g: luna_core::runtime::Gc<luna_core::runtime::Table>,
    key: luna_core::runtime::Value,
    val: luna_core::runtime::Value,
) {
    // SAFETY: the table and the values are `vm`'s (# Safety), and the heap stays in place
    unsafe { vm.heap_mut().barrier_store(g, &[key, val]) };
}

/// Write `Value::Nil` to `t[key]` (Int key). Used by
/// trace JIT when a SetList/SetI/SetTable's source register is a
/// `RegKind::Nil` (e.g. Lua's `local t = {nil, nil}` table
/// constructor expands to `NewTable; LoadNil×N; SetList` and
/// without a Nil-specific helper the existing `_table_set_int`
/// would silently coerce the Nil to `Value::Int(0)`).
///
/// Same metatable / `jit_pending_err` short-circuit as the other
/// `_table_set_*` helpers — caller deopts on `pending_err` and
/// the interpreter re-runs the op to honour `__newindex`.
///
/// No compiler emits this call any more: the trace JIT stores through
/// the `luna_jit_table_set_*_checked` helpers. It stays for the 3.x API.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table of
/// the Vm the window lends, its values that Vm's.
// SAFETY: no other item in the link is named `luna_jit_table_set_nil`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_nil(t: i64, key: i64) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit().pending_err.is_some() {
        return;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        let e = vm.runtime_error("JIT deopt: table has metatable");
        vm.set_jit_pending_err(e);
        return;
    }
    // SAFETY: `g` is a live table of the Vm lent to this call (# Safety); no reference to the table
    // is held across the store, and the heap stays in place
    let r = unsafe { g.store_int(vm.heap_mut(), key, luna_core::runtime::Value::Nil) };
    refused(vm, r);
}

/// Float-key, Float-value variant. luna 5.1 / 5.2 lower
/// `for i = 1, N do t[i] = i end` with a Float loop var (no Int
/// subtype in those dialects), so the SetTable's key and value
/// arguments arrive as f64 bit-patterns. `Table::set` normalizes
/// integral Float keys back to Int slots so `#t` still reports the
/// array length we'd expect — same shape PUC produces.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table of
/// the Vm the window lends, its values that Vm's.
// SAFETY: no other item in the link is named `luna_jit_table_set_float_float`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_float_float(
    t: i64,
    key_bits: i64,
    val_bits: i64,
) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit().pending_err.is_some() {
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety)
    let g = unsafe { table_arg(t) };
    if g.metatable().is_some() {
        let e = vm.runtime_error("JIT deopt: table has metatable");
        vm.set_jit_pending_err(e);
        return 0;
    }
    let k = luna_core::runtime::Value::Float(f64::from_bits(key_bits as u64));
    let v = luna_core::runtime::Value::Float(f64::from_bits(val_bits as u64));
    // a NaN key raises, a read-only table refuses, and a table that cannot
    // grow grows there: the interpreter re-runs the call
    // SAFETY: `g` is a live table of the Vm lent to this call (# Safety) and the key and value are
    // numbers; no reference to the table is held across the store, and the heap stays in place
    match unsafe { g.store(vm.heap_mut(), k, v) } {
        Ok(()) => 1,
        Err(luna_core::runtime::TableError::Memory(_)) => {
            vm.jit_park_oom();
            0
        }
        Err(_) => {
            let e = vm.runtime_error("JIT deopt: invalid table key");
            vm.set_jit_pending_err(e);
            0
        }
    }
}
