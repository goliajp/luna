//! The trace JIT's generic `for` call.

use crate::{current_jit_vm, push_ssa_roots};

/// Trace JIT helper for a generic `TForCall A 0 C` of any layout; `nvars`
/// packs C with the layout's registers (`ForLayout::pack_call`).
///
/// Mirrors the interpreter's `TForCall`:
/// - copies the iterator, state and control to the first loop variable's
///   register and the two after it, resizing `vm.stack` if needed
/// - calls `vm.begin_call` there to dispatch the iterator function
///
/// Restriction: the iterator at `R[A]` must be `Value::Native`. A
/// Lua-closure iter would push a Lua frame mid-trace, breaking the
/// trace head's `recording_frame_base` invariant; we deopt instead
/// (sets `jit_pending_err`, returns sentinel).
///
/// Returns `0` on success, `-1` on deopt (pending_err set OR
/// pre-existing pending_err).
///
/// A native iterator can allocate, call back into Lua and collect, so
/// `roots` carries the collectable values the trace holds only in
/// registers (see `push_ssa_roots`).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `ctrl_out`, `key_out` and
/// `val_out` are each valid for writing one `i64`; `roots` is 0 or the address of a root list as
/// `push_ssa_roots` takes it.
// SAFETY: no other item in the link is named `luna_jit_op_tforcall`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_tforcall(
    abs_offset: i64,
    nvars: i64,
    ctrl_out: *mut i64,
    key_out: *mut i64,
    val_out: *mut i64,
    roots: i64,
) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call, the
    // three out-pointers are each valid for one `i64` for the length of this call, and `roots`
    // is 0 or a root list
    let (vm, ctrl, key, val, mark) = unsafe {
        let vm = current_jit_vm();
        let mark = push_ssa_roots(vm, roots);
        (vm, &mut *ctrl_out, &mut *key_out, &mut *val_out, mark)
    };
    let r = vm.jit_op_tforcall(abs_offset as u32, nvars as i32, ctrl, key, val);
    vm.jit.ssa_roots.truncate(mark);
    r
}
