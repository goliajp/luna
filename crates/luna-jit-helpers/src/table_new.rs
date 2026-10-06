//! Table allocation helpers: constructor tables, their list stores' array
//! growth, and sunk tables materialised at a side exit.

use crate::{current_jit_vm, table_arg};

/// Allocate an empty `Gc<Table>` on the active Vm's heap.
/// Returns the Gc pointer pun'd to `i64`. Allocating never collects, but
/// the fresh table lives only in a register of the compiled code until
/// the code stores it somewhere the collector sees. Compiled code that
/// later calls a helper which can collect (a trace's concat or
/// generic-for call) passes such registers to it as roots.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_new_table`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_new_table() -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    // A prior helper in this JIT entry parked a deopt
    // request; short-circuit so we don't touch the heap unnecessarily.
    // Returning a NULL ptr is safe because subsequent helpers also
    // early-return on `jit_pending_err`, and the dispatcher will deopt
    // to the interpreter as soon as the JIT entry returns.
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let g = vm.heap.new_table();
    g.as_ptr() as i64
}

/// A table sized as `NewTable` with the packed operands `ops` (`B` in
/// bits 0–7, `C` in 8–15, `k` in bit 16; see
/// `luna_core::runtime::table::new_table_sizes`) sizes it: the trace and
/// method JIT's constructor tables, and the method JIT's presized
/// `for i = 1, N` fill, which passes the size PUC's doubling ends at.
/// The compilers only emit operands within a table's limits.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_new_table_sized`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_new_table_sized(ops: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let (b, c, k) = unpack_table_ops(ops);
    match vm.heap.new_table_presized(b, c, k) {
        Some(g) => g.as_ptr() as i64,
        None => {
            vm.jit.pending_err = Some(vm.rt_err("table overflow"));
            0
        }
    }
}

/// `NewTable`'s `(B, C, k)` from the packed form compiled code passes.
fn unpack_table_ops(ops: i64) -> (u32, u32, bool) {
    (
        (ops & 0xFF) as u32,
        ((ops >> 8) & 0xFF) as u32,
        ops >> 16 & 1 != 0,
    )
}

/// Before compiled code stores a constructor's list items up to index
/// `last` (`SetList`): grow `t`'s array part to exactly `last` when it is
/// smaller, as the interpreter's `SetList` does, so the stores that follow
/// land in it.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `t` is a live table.
// SAFETY: no other item in the link is named `luna_jit_table_reserve_list`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_reserve_list(t: i64, last: i64) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    // SAFETY: `t` is a live table (# Safety); `reserve_list` never runs the
    // collector, and nothing else refers into the table meanwhile
    let table = unsafe { table_arg(t).as_mut() };
    if table
        .reserve_list(&mut vm.heap, last.max(0) as u64)
        .is_err()
    {
        vm.jit.pending_err = Some(vm.rt_err("table overflow"));
    }
}

/// Materialize a Sinkable site's virtual array slots into
/// a heap `Gc<Table>` at a side-exit emit point. The JIT emit lays
/// out two parallel stack buffers per site per exit (`raws_ptr` of
/// `cap` × u64 and `kinds_ptr` of `cap` × u8, one entry per virt
/// slot) and calls this helper. The caller writes the returned
/// `Value::Table` raw bits into the slot's `reg_state` cell + sets
/// the per-exit-tags entry to `ExitTag::Table` so the dispatcher
/// repacks correctly on deopt.
///
/// `kind` byte uses the same `luna_core::runtime::value::raw::*` tag
/// space as `Value::pack`. Unset slots in `virt_kinds` map to
/// `raw::NIL` at emit time so the table sees a NIL fill — matches
/// Lua's "table created with array part, slot unwritten" semantics.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `raws_ptr` and
/// `kinds_ptr` point at `cap` readable entries and the three `hash_*` pointers at `n_hash`, entry
/// `i` of each pair being one value's payload and tag, and each hash key an interned string.
// SAFETY: no other item in the link is named `luna_jit_materialize_sunk_table`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_materialize_sunk_table(
    ops: i64,
    cap: i64,
    raws_ptr: *const u64,
    kinds_ptr: *const u8,
    n_hash: i64,
    hash_keys_ptr: *const u64,
    hash_raws_ptr: *const u64,
    hash_kinds_ptr: *const u8,
) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to
    // this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let cap_u = if cap > 0 { cap as usize } else { 0 };
    let n_hash_u = if n_hash > 0 { n_hash as usize } else { 0 };
    // sized by the site's `NewTable`, as the interpreter would have made it
    let (b, c, k) = unpack_table_ops(ops);
    let Some(g) = vm.heap.new_table_presized(b, c, k) else {
        vm.jit.pending_err = Some(vm.rt_err("table overflow"));
        return 0;
    };
    // SAFETY: `g` was allocated just above and nothing else refers to it
    // yet; `Table::set` never runs the collector
    let table = unsafe { g.as_mut() };
    // Array slots.
    for i in 0..cap_u {
        // SAFETY: `i < cap`, and the first `cap` entries of the two
        // buffers are readable and hold one value's payload and tag
        // (# Safety)
        let v = unsafe {
            let raw = luna_core::runtime::value::RawVal {
                zero: *raws_ptr.add(i),
            };
            luna_core::runtime::Value::pack(*kinds_ptr.add(i), raw)
        };
        let _ = table.set_int(&mut vm.heap, (i + 1) as i64, v);
    }
    // Hash slots. Each entry is a
    // (key_ptr: *const LuaStr, raw_bits, kind_byte) triple from
    // the trace IR's stack-allocated buffers. The IR baked the
    // const-string ptr at compile time from head_proto.consts.
    for i in 0..n_hash_u {
        // SAFETY: `i < n_hash`, and the first `n_hash` entries of the three
        // buffers are readable and hold an interned key and one value's
        // payload and tag (# Safety)
        let (key, v) = unsafe {
            let key = *hash_keys_ptr.add(i) as *mut luna_core::runtime::LuaStr;
            let raw = luna_core::runtime::value::RawVal {
                zero: *hash_raws_ptr.add(i),
            };
            (
                luna_core::runtime::Gc::from_ptr(key),
                luna_core::runtime::Value::pack(*hash_kinds_ptr.add(i), raw),
            )
        };
        let key = luna_core::runtime::Value::Str(key);
        let _ = table.set(&mut vm.heap, key, v);
    }
    g.as_ptr() as i64
}
