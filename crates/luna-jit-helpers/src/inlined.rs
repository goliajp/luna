//! Helpers for the frames of functions a trace inlines.

use crate::{current_jit_vm, stack_ops::new_closure};

/// Set the stack top to register `rel` of the trace's head frame (see
/// `Vm::jit_set_top`).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_set_top`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_set_top(rel: i64) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_set_top(rel as u32);
}

/// `Op::Closure A Bx` in a frame of a function the trace inlined: `cl_raw`
/// is the payload of the closure running that frame, whose registers start
/// at register `frame_off` of the trace's head frame. The trace spilled
/// the values of the in-stack upvalues to the stack first. Returns the new
/// closure's payload (0 after an earlier helper parked an error).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread;
/// `cl_raw` is the payload of a live Lua closure whose proto has a nested
/// function `proto_idx`.
// SAFETY: no other item in the link is named `luna_jit_op_closure_in`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_closure_in(
    cl_raw: i64,
    proto_idx: i64,
    frame_off: i64,
) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: `cl_raw` is the payload of a live Lua closure (# Safety)
    let cl =
        unsafe { luna_core::runtime::Gc::from_ptr(cl_raw as *mut luna_core::runtime::LuaClosure) };
    let Some(head) = vm.jit_last_lua_frame() else {
        vm.jit.pending_err = Some(vm.rt_err("JIT op_closure: no Lua frame"));
        return 0;
    };
    new_closure(vm, cl, proto_idx as usize, head.base + frame_off as u32)
}
