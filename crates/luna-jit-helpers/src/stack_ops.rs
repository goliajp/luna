//! Stack-slot access and the opcode helpers that work on the frame (close, concat, tforcall, closure).

use crate::{current_jit_closure, current_jit_vm};

/// Trace JIT helper for `Op::Close A`. Wraps
/// `Vm::jit_op_close` which does the predict-and-deopt logic:
/// returns 0 to continue the trace, 1 to deopt (handler would run
/// or pre-existing pending_err).
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_close(start_offset: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
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
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_stack_update_raw(slot_offset: i64, raw_bits: i64) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    vm.jit_stack_update_raw(slot_offset as u32, raw_bits as u64);
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
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_op_concat(slot_offset: i64, n: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    vm.jit_op_concat(slot_offset as u32, n as i32)
}

/// Trace JIT helper for `Op::TForCall A 0 C`.
///
/// Mirrors `exec.rs:5316` Op::TForCall semantics:
/// - copies `R[A..=A+2]` (iter / state / control) to `R[A+4..=A+6]`,
///   resizing `vm.stack` if needed
/// - calls `vm.begin_call(abs+4, Some(2), nvars, false)` to dispatch
///   the iterator function
///
/// Restriction: the iterator at `R[A]` must be `Value::Native`. A
/// Lua-closure iter would push a Lua frame mid-trace, breaking the
/// trace head's `recording_frame_base` invariant; we deopt instead
/// (sets `jit_pending_err`, returns sentinel).
///
/// Returns `0` on success, `-1` on deopt (pending_err set OR
/// pre-existing pending_err).
///
/// Safety: caller (trace JIT IR) runs under `enter_jit` so
/// `current_jit_vm()` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_tforcall(
    abs_offset: i64,
    nvars: i64,
    ctrl_out: *mut i64,
    key_out: *mut i64,
    val_out: *mut i64,
) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    vm.jit_op_tforcall(abs_offset as u32, nvars as i32, ctrl_out, key_out, val_out)
}

/// Load the raw `i64` payload of `vm.stack[base + slot_offset]`
/// for the active trace's head frame. Used to reload trace IR
/// `Variable`s after a helper (e.g. `luna_jit_op_tforcall`) has
/// mutated `vm.stack` directly.
///
/// Safety: caller (trace JIT IR) runs under `enter_jit` so
/// `current_jit_vm()` is live. Returns `0` if the slot is out of
/// stack range (defensive — emit-time bounds check should make this
/// unreachable).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_stack_load(slot_offset: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    vm.jit_stack_load(slot_offset as u32)
}

/// Read the tag byte of `vm.stack[base + slot_offset]`
/// for the active trace's head frame. Used by `Op::TForLoop` emit
/// to dispatch on the iterator's return-key tag (Nil → loop end,
/// Int → continue for ipairs, other → deopt for v2).
///
/// Safety: caller (trace JIT IR) runs under `enter_jit`. Returns
/// `raw::NIL` (0) if slot out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_stack_tag(slot_offset: i64) -> i64 {
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
/// Safety: caller (trace JIT IR) runs under `enter_jit` so
/// `current_jit_vm()` is live; the (tag, raw_bits) pair is
/// generated by the same emit path that proves the kind, so
/// `Value::pack` round-trips correctly.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_spill_to_stack(slot_offset: i64, tag: i64, raw_bits: i64) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    vm.jit_spill_stack(slot_offset as u32, tag as u8, raw_bits as u64);
}

/// Trace JIT helper for `Op::Closure A Bx`.
///
/// Looks up `cl.proto.protos[bx]` (the inner Proto) and builds a
/// new `Gc<LuaClosure>` for it. Each upval is captured either from
/// the trace head closure's `upvals()` slice (`in_stack=false`)
/// or from the caller frame's stack via `find_or_create_upval`
/// (`in_stack=true`). v51 dialect clones the `_ENV` cell
/// to match interp semantics (per-closure `_ENV`). v52+ honours
/// the Proto cache.
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
/// Safety: caller runs under `enter_jit(vm, Some(cl))` guard so
/// `current_jit_vm()` / `current_jit_closure()` return live
/// references. `proto_idx` is in-bounds by the emit pre-check.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_closure(proto_idx: i64) -> i64 {
    use luna_core::runtime::function::{INLINE_UPVALS_N, UpvalState, Upvalue};
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let cl = unsafe { current_jit_closure() };
    let inner = cl.proto.protos[proto_idx as usize];
    let n_ups = inner.upvals.len();
    // Determine the caller frame's base for in_stack captures. The
    // helper runs MID-trace, before any frame writeback — the trace
    // head's frame is the topmost Lua frame here (the lowerer restricts
    // Op::Closure emit to inline_depth=0 only, so no deeper frame
    // exists).
    let base = match vm.jit_last_lua_frame() {
        Some(f) => f.base,
        None => {
            vm.jit.pending_err = Some(vm.rt_err("JIT op_closure: no Lua frame"));
            return 0;
        }
    };
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
        // SAFETY: first n_ups slots of stack_buf were initialised
        // by the loop above; we expose exactly that range.
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
    let ups_slice: &[luna_core::runtime::Gc<Upvalue>] = ups;
    let nc = if v51 {
        vm.heap.new_closure_inline(inner, ups_slice)
    } else {
        // PUC 5.2+ getcached: reuse the last LuaClosure built for
        // this Proto if every upval slot points to the same
        // Upvalue object (typical for `function() return outer end`
        // captured inside a hot loop).
        let cached = inner.cache.get().filter(|c| {
            c.upvals().len() == ups_slice.len()
                && c.upvals()
                    .iter()
                    .zip(ups_slice.iter())
                    .all(|(a, b)| std::ptr::eq(a.as_ptr(), b.as_ptr()))
        });
        match cached {
            Some(c) => c,
            None => {
                let n = vm.heap.new_closure_inline(inner, ups_slice);
                inner.cache.set(Some(n));
                n
            }
        }
    };
    let (_tag, raw) = luna_core::runtime::Value::Closure(nc).unpack();
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    unsafe { raw.zero as i64 }
}
