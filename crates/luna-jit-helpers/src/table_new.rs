//! Table allocation helpers: constructor tables, their list stores' array
//! growth, and sunk tables materialised at a side exit.

use crate::{current_jit_vm, table_arg};
use luna_core::__private::VmJitState;
use luna_core::__private::{GcInternals, HeapInternals, TableInternals, VmJitAlloc};

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
    if vm.jit().pending_err.is_some() {
        return 0;
    }
    match vm.heap.try_new_table() {
        Ok(g) => g.as_ptr() as i64,
        // compiled code leaves at the `NewTable`, and the interpreter
        // allocates, collecting in an emergency as compiled code may not
        Err(_) => {
            vm.jit_park_oom();
            0
        }
    }
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
    if vm.jit().pending_err.is_some() {
        return 0;
    }
    let (b, c, k) = unpack_table_ops(ops);
    match vm.heap.try_new_table_presized(b, c, k) {
        Ok(Some(g)) => g.as_ptr() as i64,
        Ok(None) => {
            vm.jit_mut().pending_err = Some(vm.runtime_error("table overflow"));
            0
        }
        // the interpreter runs the `NewTable` again and allocates
        Err(_) => {
            vm.jit_park_oom();
            0
        }
    }
}

/// The method JIT's `NewTable` of the packed operands `ops`, made with the
/// size `presized` gives when a fill follows (counted as the size PUC's
/// doubling ends at; no collection check comes before the fill ends). 0
/// when the op is the interpreter's: its check would find a collection
/// step due, or the table cannot be made; the level then hands itself
/// over and the interpreter runs the op.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_chunk_new_table`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_chunk_new_table(ops: i64, presized: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    let (b, c, k) = unpack_table_ops(ops);
    if vm.jit().pending_err.is_some() || vm.jit_table_due(b, c, k) {
        return 0;
    }
    let (b, c, k) = unpack_table_ops(presized);
    match vm.heap.try_new_table_presized(b, c, k) {
        Ok(Some(g)) => g.as_ptr() as i64,
        _ => 0,
    }
}

/// `NewTable`'s `(B, C, k)` from the packed form compiled code passes.
pub(crate) fn unpack_table_ops(ops: i64) -> (u32, u32, bool) {
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
    if vm.jit().pending_err.is_some() {
        return;
    }
    // SAFETY: `t` is a live table (# Safety); `reserve_list` never runs the
    // collector, and nothing else refers into the table meanwhile
    let table = unsafe { table_arg(t).as_mut() };
    match table.reserve_list(&mut vm.heap, last.max(0) as u64) {
        Ok(()) => {}
        Err(luna_core::runtime::table::TableError::Overflow) => {
            vm.jit_mut().pending_err = Some(vm.runtime_error("table overflow"));
        }
        // the interpreter runs the `SetList` again and grows the table
        Err(_) => vm.jit_park_oom(),
    }
}
