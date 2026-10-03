//! The trace JIT's string accumulator buffers (`s = s .. v` loops).

use crate::current_jit_vm;

/// Trace JIT helper:acquire a fresh accumulator
/// buffer from the Vm's pool. Returns a `*mut Vec<u8>` boxed-leaked
/// pointer that the trace fn keeps in a stack slot through the loop.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread. The buffer goes back
/// through `luna_jit_str_buf_release`.
// SAFETY: no other item in the link is named `luna_jit_str_buf_acquire`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_acquire() -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_acquire() as i64
}

/// Trace JIT helper:release a buffer back to the
/// Vm's pool.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `buf` came from
/// `luna_jit_str_buf_acquire` on the same Vm and is not used again.
// SAFETY: no other item in the link is named `luna_jit_str_buf_release`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_release(buf: i64) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_release(buf as *mut Vec<u8>);
}

/// Trace JIT helper:append a LuaStr's bytes to a
/// previously-acquired accumulator buffer. The trace IR calls this
/// at each loop iter inside the `s = s .. v` idiom.
///
/// Returns 0 on success, -1 if `str_ptr` isn't a valid LuaStr (deopt
/// to interp, which will hit the __concat metamethod path).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `buf` came from
/// `luna_jit_str_buf_acquire` on the same Vm and has not been released, and `str_ptr` is 0 or a
/// live string.
// SAFETY: no other item in the link is named `luna_jit_str_buf_extend`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_extend(buf: i64, str_ptr: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_extend(buf as *mut Vec<u8>, str_ptr)
}

/// Trace JIT helper:drain the accumulator buffer
/// into a fresh `LuaStr` via `heap.intern`, returning the raw ptr
/// bits for the trace to write into the accumulator slot.
///
/// Returns the LuaStr ptr as i64 on success, 0 on overflow (the v2
/// hard cap = 256KB; trace deopts).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `buf` came from
/// `luna_jit_str_buf_acquire` on the same Vm and has not been released.
// SAFETY: no other item in the link is named `luna_jit_str_buf_intern`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_intern(buf: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_intern(buf as *mut Vec<u8>)
}
