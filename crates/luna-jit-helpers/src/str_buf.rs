//! The trace JIT's string accumulator buffers (`s = s .. v` loops).

use crate::current_jit_vm;

/// Trace JIT helper:acquire a fresh accumulator
/// buffer from the Vm's pool. Returns a `*mut Vec<u8>` boxed-leaked
/// pointer that the trace fn keeps in a stack slot through the loop.
///
/// Safety: caller must be inside `enter_jit` and must eventually call
/// `luna_jit_str_buf_release` with the returned pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_acquire() -> i64 {
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_acquire() as i64
}

/// Trace JIT helper:release a buffer back to the
/// Vm's pool.
///
/// Safety: `buf` must have been returned by a prior
/// `luna_jit_str_buf_acquire` on the same Vm.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_release(buf: i64) {
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
/// Safety: `buf` from prior `acquire`; `str_ptr` from the piece slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_extend(buf: i64, str_ptr: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
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
/// Safety: `buf` from prior `acquire`. The buffer is drained and
/// ready for `release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_intern(buf: i64) -> i64 {
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_intern(buf as *mut Vec<u8>)
}
