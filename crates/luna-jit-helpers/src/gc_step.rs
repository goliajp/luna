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
/// buffer.
// SAFETY: no other item in the link is named `luna_jit_gc_due`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_gc_due(op_pc: i64, exit_ret: i64, reg_state: *const i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    let n = vm.jit_step_window(exit_ret as u64);
    // SAFETY: a window is given only for the running trace's frame, whose
    // register buffer (# Safety) holds at least that frame's registers
    let regs = (n > 0).then(|| unsafe { std::slice::from_raw_parts(reg_state, n) });
    vm.jit_gc_step(op_pc as u32, exit_ret as u64, regs)
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
