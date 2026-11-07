//! Collection steps from compiled code.

use crate::current_jit_vm;
use luna_core::__private::VmJitAlloc;

/// Compiled code reached the point a collection step falls due at, after
/// the op whose check `op_pc` names (see `Vm::checkpoint_after_jit`).
/// `exit_ret` 0: 1 when the step is due, compiled code then leaving after
/// its op for the interpreter to check, 0 otherwise. Otherwise the exit a
/// trace leaves through there, with its frame's registers stored at
/// `reg_state` as that exit leaves them: the step runs here when due; 0
/// when the trace goes on, else it leaves through that exit.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread;
/// with `exit_ret` not 0, from the running trace, `reg_state` its register
/// buffer (its window and the words past it); with `exit_ret` 0 it is not
/// read and may be null.
// SAFETY: no other item in the link is named `luna_jit_gc_due`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_gc_due(op_pc: i64, exit_ret: i64, reg_state: *const i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to
    // this call; with `exit_ret` not 0 the call comes from the running trace
    // and `reg_state` is its register buffer, as `jit_gc_step` needs
    unsafe { current_jit_vm().jit_gc_step(op_pc as u32, exit_ret as u64, reg_state) }
}

/// Where the count and the point a collection step falls due at lie
/// (`VmJitAlloc::jit_due_words`): compiled code loads it once on entry and
/// compares the two words itself before it calls [`luna_jit_gc_due`].
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_due_words`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_due_words() -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_due_words() as i64
}

/// Runs the list `list` of one of the running trace's side cells: the
/// dispatcher's checks before the side trace runs (with the stack's tags
/// `list` names; none when null), or its stores to the stack when it goes
/// on in the trace's loop (see `luna_core::__private::SideCell`). 1 when
/// the checks pass, or after the stores; 0 otherwise.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread,
/// by the running trace: `list` is null or a list of one of its side cells
/// (its kind, its length, the registers it reaches, then its entries) and
/// `reg_state` its register buffer, at least that many registers long.
// SAFETY: no other item in the link is named `luna_jit_side_cell_list`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_side_cell_list(list: *const u32, reg_state: *mut i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to
    // this call; `list`, unless null, starts with its kind, its length and
    // the registers its entries reach, which `reg_state` holds
    let (vm, list, regs) = unsafe {
        let (n, len) = match list.is_null() {
            true => (0, 0),
            false => (3 + *list.add(1) as usize, *list.add(2) as usize),
        };
        (
            current_jit_vm(),
            std::slice::from_raw_parts(if n == 0 { std::ptr::dangling() } else { list }, n),
            std::slice::from_raw_parts_mut(
                if len == 0 {
                    std::ptr::dangling_mut()
                } else {
                    reg_state
                },
                len,
            ),
        )
    };
    vm.jit_side_cell_list(list, regs)
}
