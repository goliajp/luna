//! Table store helpers.

use crate::{current_jit_vm, str_arg, table_arg};
use luna_core::__private::VmJitState;
use luna_core::__private::{GcInternals, TableInternals, ValueInternals, VmJitAlloc};

/// `t[key] = val` where `t` is a Table Gc (i64 pun), `key`
/// is an Int and `val` is an Int. Wraps `Table::set_int(&mut Heap,
/// i64, Value)`. Returns 1 when stored, 0 when it parked a deopt (a
/// metatable, or a table that could not grow); the method JIT's code
/// then returns at once and the interpreter runs the call.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table.
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
        vm.jit_mut().pending_err = Some(vm.runtime_error("JIT deopt: table has metatable"));
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety); `g` is not dereferenced again
    // while `table` is in use, and `Table::set*` never runs the collector
    let table = unsafe { g.as_mut() };
    // the method JIT stores only into tables its call tested on entry or
    // made itself (see `Vm::try_jit_call_op`)
    let r = table.set_int_unguarded(&mut vm.heap, key, luna_core::runtime::Value::Int(val));
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
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table, and
/// `tag` and `raw_bits` are one value's tag and payload.
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
        vm.jit_mut().pending_err = Some(vm.runtime_error("JIT deopt: table has metatable"));
        return 0;
    }
    // SAFETY: `t` is a live table and (`tag`, `raw_bits`) one value's tag
    // and payload (# Safety); `g` is not dereferenced again while `table`
    // is in use, and `Table::set_int` never runs the collector
    let (table, v) = unsafe {
        let raw = luna_core::__private::RawVal {
            zero: raw_bits as u64,
        };
        (g.as_mut(), luna_core::runtime::Value::pack(tag as u8, raw))
    };
    let r = table.set_int(&mut vm.heap, key, v);
    if refused(vm, r) {
        return 0;
    }
    barrier_for(vm, g, luna_core::runtime::Value::Int(key), v);
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
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table,
/// `key_ptr` an interned string, and `val_tag` and `val_raw` one value's tag and payload.
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
        vm.jit_mut().pending_err = Some(vm.runtime_error("JIT deopt: table has metatable"));
        return;
    }
    // SAFETY: `key_ptr` is an interned string (# Safety)
    let key_gc = unsafe { str_arg(key_ptr) };
    let key = luna_core::runtime::Value::Str(key_gc);
    // SAFETY: `t` is a live table and (`val_tag`, `val_raw`) one value's
    // tag and payload (# Safety); `g` is not dereferenced again while
    // `table` is in use, and `Table::set` never runs the collector
    let (table, v) = unsafe {
        let raw = luna_core::__private::RawVal {
            zero: val_raw as u64,
        };
        (
            g.as_mut(),
            luna_core::runtime::Value::pack(val_tag as u8, raw),
        )
    };
    let r = table.set(&mut vm.heap, key, v);
    if refused(vm, r) {
        return;
    }
    barrier_for(vm, g, key, v);
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
            vm.jit_mut().pending_err = Some(vm.runtime_error("JIT deopt: table is read-only"));
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
pub(crate) fn barrier_for(
    vm: &mut luna_core::vm::Vm,
    g: luna_core::runtime::Gc<luna_core::runtime::Table>,
    key: luna_core::runtime::Value,
    val: luna_core::runtime::Value,
) {
    vm.heap.barrier_store(g, &[key, val]);
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
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table.
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
        vm.jit_mut().pending_err = Some(vm.runtime_error("JIT deopt: table has metatable"));
        return;
    }
    // SAFETY: `t` is a live table (# Safety); `g` is not dereferenced again
    // while `table` is in use, and `Table::set*` never runs the collector
    let table = unsafe { g.as_mut() };
    let r = table.set_int(&mut vm.heap, key, luna_core::runtime::Value::Nil);
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
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table.
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
        vm.jit_mut().pending_err = Some(vm.runtime_error("JIT deopt: table has metatable"));
        return 0;
    }
    // SAFETY: `t` is a live table (# Safety); `g` is not dereferenced again
    // while `table` is in use, and `Table::set*` never runs the collector
    let table = unsafe { g.as_mut() };
    let k = luna_core::runtime::Value::Float(f64::from_bits(key_bits as u64));
    let v = luna_core::runtime::Value::Float(f64::from_bits(val_bits as u64));
    // a NaN key raises, and a table that cannot grow grows there: the
    // interpreter re-runs the call
    match table.set_unguarded(&mut vm.heap, k, v) {
        Ok(()) => 1,
        Err(luna_core::runtime::TableError::Memory(_)) => {
            vm.jit_park_oom();
            0
        }
        Err(_) => {
            vm.jit_mut().pending_err = Some(vm.runtime_error("JIT deopt: invalid table key"));
            0
        }
    }
}
