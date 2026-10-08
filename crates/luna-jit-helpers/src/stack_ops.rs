//! Stack-slot access and the opcode helpers that work on the frame (close, concat, tforcall, closure).

use crate::{current_jit_closure, current_jit_vm, payload_bits, push_ssa_roots};

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

/// Update only the raw payload of
/// `vm.stack[base + slot_offset]`, preserving its existing tag.
/// Used by `Op::Concat` body emit to spill trace-IR Variables
/// back to vm.stack for operands whose `current_kinds` is
/// `Unset` (e.g. Str slots that round-trip as pointer raw bits
/// but have no `RegKind::Str` variant). The interp's previous
/// execution of the same op already wrote the right `tag` to
/// that slot — the trace just needs to refresh the raw bits.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `raw_bits` is the
/// payload of a value of the type the slot holds now.
// SAFETY: no other item in the link is named `luna_jit_stack_update_raw`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_stack_update_raw(slot_offset: i64, raw_bits: i64) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    if let Some(slot) = vm.jit_stack_slot_mut(slot_offset as u32) {
        let (tag, _) = slot.unpack();
        let raw = luna_core::runtime::value::RawVal {
            zero: raw_bits as u64,
        };
        // SAFETY: `tag` is the slot's own tag and `raw_bits` a payload of that type (# Safety)
        *slot = unsafe { luna_core::runtime::Value::pack(tag, raw) };
    }
}

/// Trace JIT helper for `Op::Concat A B`.
///
/// Wraps `Vm::jit_op_concat` which mirrors the interp arm: sets
/// `self.top = base + a + n`, then runs `concat_run(base + a)`.
/// Detects metamethod-path (which would push a Lua frame mid-trace)
/// via pre/post `frames.len()` comparison and deopts cleanly via
/// `pending_err` + frame unwind.
///
/// Returns `0` on success (result lives at `vm.stack[base + a]`),
/// `-1` on deopt (pending_err set; metamethod path, type error,
/// length overflow, or pre-existing pending_err).
///
/// The concat steps the collector, so `roots` carries the collectable
/// values the trace holds only in registers (see `push_ssa_roots`).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `roots` is 0 or the
/// address of a root list as `push_ssa_roots` takes it.
// SAFETY: no other item in the link is named `luna_jit_op_concat`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_concat(slot_offset: i64, n: i64, roots: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call, and
    // `roots` is 0 or a root list (# Safety)
    let (vm, mark) = unsafe {
        let vm = current_jit_vm();
        let mark = push_ssa_roots(vm, roots);
        (vm, mark)
    };
    let r = vm.jit_op_concat(slot_offset as u32, n as i32);
    vm.jit.ssa_roots.truncate(mark);
    r
}

/// Load the raw `i64` payload of `vm.stack[base + slot_offset]`
/// for the active trace's head frame. Used to reload trace IR
/// `Variable`s after a helper (e.g. `luna_jit_op_tforcall`) has
/// mutated `vm.stack` directly.
///
/// Returns `0` if the slot is out of stack range (defensive —
/// emit-time bounds check should make this unreachable).
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_stack_load`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_stack_load(slot_offset: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_stack_load(slot_offset as u32)
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
    if vm.jit.pending_err.is_some() {
        return;
    }
    let raw = luna_core::runtime::value::RawVal {
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
/// payload). On error (`pending_err` already set) returns 0
/// sentinel so the dispatcher deopts.
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
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // the trace head's frame is the topmost Lua frame while the trace runs
    // (inlined frames are not pushed)
    let base = match vm.jit_last_lua_frame() {
        Some(f) => f.base,
        None => {
            vm.jit.pending_err = Some(vm.rt_err("JIT op_closure: no Lua frame"));
            return 0;
        }
    };
    new_closure(vm, cl, proto_idx as usize, base)
}

/// `cl.proto.protos[idx]` as a new closure of a frame of `cl` whose
/// registers start at stack slot `base`: in-stack upvalues are opened on
/// that frame's slots (the trace spilled their values first), the others
/// taken from `cl`; 5.1 gives the closure its own `_ENV` cell and 5.2 /
/// 5.3 reuse the proto's cached closure, as the interpreter does. Returns
/// the closure's payload.
pub(crate) fn new_closure(
    vm: &mut luna_core::vm::Vm,
    cl: luna_core::runtime::Gc<luna_core::runtime::LuaClosure>,
    idx: usize,
    base: u32,
) -> i64 {
    use luna_core::runtime::function::{INLINE_UPVALS_N, UpvalState, Upvalue};
    let inner = cl.proto.protos[idx];
    let n_ups = inner.upvals.len();
    // Build the upval slice — small (0..2 typical) so use a stack
    // array up to INLINE_UPVALS_N like the interp does, else heap.
    let mut stack_buf: [std::mem::MaybeUninit<luna_core::runtime::Gc<Upvalue>>; INLINE_UPVALS_N] =
        [std::mem::MaybeUninit::uninit(); INLINE_UPVALS_N];
    let mut heap_buf: Vec<luna_core::runtime::Gc<Upvalue>> = Vec::new();
    let use_inline = n_ups <= INLINE_UPVALS_N;
    if !use_inline {
        heap_buf.reserve_exact(n_ups);
    }
    for (i, d) in inner.upvals.iter().enumerate() {
        let uv = if d.in_stack {
            // `find_or_create_upval` points the open
            // upval at vm.stack[base + d.index]. The trace IR
            // emitted a spill before this call, so the slot holds
            // the right value at capture time.
            vm.find_or_create_upval(base + d.index as u32)
        } else {
            cl.upvals()[d.index as usize]
        };
        if use_inline {
            stack_buf[i] = std::mem::MaybeUninit::new(uv);
        } else {
            heap_buf.push(uv);
        }
    }
    let ups: &mut [luna_core::runtime::Gc<Upvalue>] = if use_inline {
        // SAFETY: `use_inline` means `n_ups <= INLINE_UPVALS_N`, and the
        // loop above wrote `stack_buf[i]` for every `i < n_ups`
        // (`inner.upvals` has `n_ups` entries), so the slice covers only
        // initialised slots
        unsafe {
            std::slice::from_raw_parts_mut(
                stack_buf.as_mut_ptr() as *mut luna_core::runtime::Gc<Upvalue>,
                n_ups,
            )
        }
    } else {
        &mut heap_buf[..]
    };
    // v51 per-closure `_ENV` clone — matches interp Op::Closure.
    let v51 = vm.version() <= luna_core::version::LuaVersion::Lua51;
    if v51 && inner.env_upval_idx != u8::MAX {
        let i = inner.env_upval_idx as usize;
        let cur = match ups[i].state() {
            UpvalState::Open { slot, thread } => vm.read_slot(slot, thread),
            UpvalState::Closed(v) => v,
        };
        ups[i] = vm.heap.new_upvalue(UpvalState::Closed(cur));
    }
    let nc = vm.closure_from_proto(inner, ups);
    payload_bits(luna_core::runtime::Value::Closure(nc))
}
