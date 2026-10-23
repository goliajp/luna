//! The trace JIT's string accumulator buffers (`s = s .. v` loops).

use crate::{current_jit_vm, str_arg};
use luna_core::__private::VmJitAlloc;
use luna_core::runtime::mem::LVec;

/// Trace JIT helper:acquire a fresh accumulator
/// buffer from the Vm's pool. Returns a `*mut LVec<u8>` boxed-leaked
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
    Box::into_raw(vm.jit_str_buf_acquire()) as i64
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
    if buf == 0 {
        return;
    }
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call, and `buf`
    // is the box `luna_jit_str_buf_acquire` leaked, released once, so this takes back ownership
    let (vm, buf) = unsafe { (current_jit_vm(), Box::from_raw(buf as *mut LVec<u8>)) };
    vm.jit_str_buf_release(buf);
}

/// Trace JIT helper:append a LuaStr's bytes to a
/// previously-acquired accumulator buffer. The trace IR calls this
/// at each loop iter inside the `s = s .. v` idiom.
///
/// Returns 0 on success, -1 when `buf` or `str_ptr` is 0 or the buffer
/// cannot grow (the trace exits at the concatenation and the interpreter
/// performs it). Nothing here checks that `str_ptr` is a string: the
/// trace passes only string registers.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `buf` is 0 or came from
/// `luna_jit_str_buf_acquire` on the same Vm and has not been released, and `str_ptr` is 0 or a
/// live string.
// SAFETY: no other item in the link is named `luna_jit_str_buf_extend`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_extend(buf: i64, str_ptr: i64) -> i64 {
    if buf == 0 || str_ptr == 0 {
        return -1;
    }
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call; `buf` is a
    // live boxed buffer only this call uses, and `str_ptr` a live string
    let (vm, buf, s) = unsafe {
        (
            current_jit_vm(),
            &mut *(buf as *mut LVec<u8>),
            str_arg(str_ptr),
        )
    };
    if vm.jit_str_buf_extend(buf, s) { 0 } else { -1 }
}

/// The `CONCAT` at `op_pc` of a trace's accumulated concatenation: the
/// piece appended and PUC's string of the two counted (see
/// `Vm::jit_str_buf_concat`). 0 on success, -1 as
/// [`luna_jit_str_buf_extend`], 1 when a collection step falls due.
///
/// # Safety
/// As [`luna_jit_str_buf_extend`].
// SAFETY: no other item in the link is named `luna_jit_str_buf_concat`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_concat(buf: i64, str_ptr: i64, op_pc: i64) -> i64 {
    if buf == 0 || str_ptr == 0 {
        return -1;
    }
    // SAFETY: as in `luna_jit_str_buf_extend`
    let (vm, buf, s) = unsafe {
        (
            current_jit_vm(),
            &mut *(buf as *mut LVec<u8>),
            str_arg(str_ptr),
        )
    };
    vm.jit_str_buf_concat(buf, s, op_pc as u32)
}

/// Trace JIT helper:drain the accumulator buffer
/// into a fresh `LuaStr` via `heap.intern`, returning the raw ptr
/// bits for the trace to write into the accumulator slot.
///
/// Returns the LuaStr ptr as i64 on success, 0 past the 256KB hard cap or
/// when the string cannot be allocated: a deopt is parked then, and the
/// dispatcher drops the trace's registers.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `buf` came from
/// `luna_jit_str_buf_acquire` on the same Vm and has not been released.
// SAFETY: no other item in the link is named `luna_jit_str_buf_intern`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_intern(buf: i64) -> i64 {
    if buf == 0 {
        return 0;
    }
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call, and `buf`
    // is a live boxed buffer only this call uses
    let (vm, buf) = unsafe { (current_jit_vm(), &mut *(buf as *mut LVec<u8>)) };
    vm.jit_str_buf_intern(buf)
}
