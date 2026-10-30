//! Stack-slot access and the opcode helpers that work on the frame (close, concat, tforcall, closure).

use crate::{current_jit_closure, current_jit_vm, payload_bits, push_ssa_roots};
use luna_core::__private::VmJitState;
use luna_core::__private::{ValueInternals, VmJitAlloc, VmJitFrames};

/// Trace JIT helper for `Op::Close A`. Wraps
/// `Vm::jit_op_close` which does the predict-and-deopt logic:
/// returns 0 to continue the trace, 1 to deopt (handler would run
/// or pre-existing pending_err).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_op_close`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_close(start_offset: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_op_close(start_offset as u32)
}

/// Trace JIT helper for `Op::Concat`: `ops` holds the `n` operands as
/// (tag, payload) words, a tag below 0 keeping the stack slot's own, and
/// [`luna_core::__private::VmJitFrames::jit_concat_regs`] stores them from
/// register `a` on and runs the concat. Returns the result's payload, and
/// the payloads of the operands `reload` names back in `ops`; `0` when the
/// interpreter must redo the op.
///
/// The concat may collect in an emergency, so `roots` carries the
/// collectable values the trace holds only in registers (see
/// `push_ssa_roots`).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `ops` points at `2 * n`
/// writable words; `roots` is 0 or the address of a root list as `push_ssa_roots` takes it.
// SAFETY: no other item in the link is named `luna_jit_op_concat_regs`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_concat_regs(
    a: i64,
    n: i64,
    ops: i64,
    reload: i64,
    roots: i64,
) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call, `ops`
    // is `n` pairs of words and `roots` 0 or a root list (# Safety)
    let (vm, mark, ops) = unsafe {
        let vm = current_jit_vm();
        let mark = push_ssa_roots(vm, roots);
        let ops = std::slice::from_raw_parts_mut(ops as *mut [i64; 2], n as usize);
        (vm, mark, ops)
    };
    let r = vm.jit_concat_regs(a as u32, ops, reload as u64);
    vm.jit_mut().ssa_roots = mark;
    r
}

/// Read the tag byte of `vm.stack[base + slot_offset]`
/// for the active trace's head frame. Used by `Op::TForLoop` emit
/// to dispatch on the iterator's return-key tag (Nil → loop end,
/// Int → continue for ipairs, other → deopt for v2).
///
/// Returns `raw::NIL` (0) if slot out of range.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_stack_tag`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_stack_tag(slot_offset: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_stack_tag(slot_offset as u32) as i64
}

/// Spill a trace's per-register live value into the
/// caller frame's `vm.stack[base + slot_offset]`. Always called
/// just before `luna_jit_op_closure` for each `in_stack: true`
/// upval in the inner proto, so the open upval the helper creates
/// points to a slot holding the right value.
///
/// Parameters: `slot_offset` is the caller-frame register index
/// (`u32`, depth=0 only — depth>0 Closure is not supported).
/// `tag` is the `raw::*` byte for the register's RegKind at this
/// emit point (Int / Float / Table / Closure / Nil). `raw_bits` is
/// the trace IR's i64 payload for the register (Float held as
/// `f64::to_bits`, Table/Closure as raw `Gc::as_ptr` cast).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `tag` and `raw_bits` are
/// one value's tag and payload.
// SAFETY: no other item in the link is named `luna_jit_spill_to_stack`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_spill_to_stack(slot_offset: i64, tag: i64, raw_bits: i64) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit().pending_err.is_some() {
        return;
    }
    let raw = luna_core::__private::RawVal {
        zero: raw_bits as u64,
    };
    // SAFETY: `tag` and `raw_bits` are one value's tag and payload (# Safety)
    let v = unsafe { luna_core::runtime::Value::pack(tag as u8, raw) };
    vm.jit_spill_stack(slot_offset as u32, v);
}

/// Trace JIT helper for `Op::Closure A Bx`.
///
/// Looks up `cl.proto.protos[bx]` (the inner Proto) and builds a
/// new `Gc<LuaClosure>` for it. Each upval is captured either from
/// the trace head closure's `upvals()` slice (`in_stack=false`)
/// or from the caller frame's stack via `find_or_create_upval`
/// (`in_stack=true`). v51 dialect clones the `_ENV` cell
/// to match interp semantics (per-closure `_ENV`); 5.2 / 5.3 reuse
/// the Proto's cached closure as the interpreter does.
///
/// **Pre-condition for in_stack upvals**: the trace IR has already
/// emitted `luna_jit_spill_to_stack(d.index, tag, raw)` for every
/// `d.in_stack == true` upval BEFORE this call, so the underlying
/// `vm.stack[base + d.index]` holds the trace's current value at
/// helper time. Without that spill the open upval would point at
/// a stale entry-tag value.
///
/// Returns the raw `Gc<LuaClosure>` ptr as i64 (Value::Closure's
/// payload), or 0 when `pending_err` is already set or an allocation
/// fails: the trace exits at the `Closure` and the interpreter makes it.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread opened with the running
/// closure.
// SAFETY: no other item in the link is named `luna_jit_op_closure`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_closure(proto_idx: i64) -> i64 {
    // SAFETY: inside an enter_jit window opened with the running closure (# Safety) JIT_VM is the
    // Vm lent to this call and JIT_CL that closure
    let (vm, cl) = unsafe { (current_jit_vm(), current_jit_closure()) };
    if vm.jit().pending_err.is_some() {
        return 0;
    }
    // the trace head's frame is the topmost Lua frame while the trace runs
    // (inlined frames are not pushed)
    let base = match vm.jit_last_lua_frame() {
        Some(f) => f.base,
        None => {
            vm.jit_mut().pending_err = Some(vm.runtime_error("JIT op_closure: no Lua frame"));
            return 0;
        }
    };
    // SAFETY: `cl` is the running closure (# Safety), its frame the trace
    // head's at `base`; compiled code asks only for its own prototypes
    unsafe { new_closure(vm, cl, proto_idx as usize, base) }
}

/// `cl.proto.protos[idx]` as a new closure of a frame of `cl` whose
/// registers start at stack slot `base`, as the interpreter's `CLOSURE`
/// makes it (the trace spilled the in-stack upvalues' values first).
/// Returns the closure's payload, 0 when it cannot be made.
///
/// # Safety
/// `cl` runs the frame whose registers start at `base`, and its prototype
/// has a nested function `idx`.
pub(crate) unsafe fn new_closure(
    vm: &mut luna_core::vm::Vm,
    cl: luna_core::runtime::Gc<luna_core::runtime::LuaClosure>,
    idx: usize,
    base: u32,
) -> i64 {
    // SAFETY: the caller's contract
    match unsafe { vm.jit_closure(cl, idx, base) } {
        Some(nc) => payload_bits(luna_core::runtime::Value::Closure(nc)),
        None => 0,
    }
}
