//! Upvalue reads and the head-closure accessors.

use crate::table_read::checked_read;
use crate::{JIT_CL, current_jit_closure, current_jit_vm};

/// `R[A] = upvals[idx]` value-read variant. Reads the
/// active closure's upvalue cell, dispatching open/closed via the
/// interpreter's `Vm::upval_get` (so an open upvalue resolves to its
/// current stack slot — matters when a closure is called from inside
/// an enclosing function whose upvalues are still open). Returns the
/// raw 8-byte payload (same convention as the table helpers): the
/// JIT-emitted caller bitcasts to F64 if the slot's declared kind is
/// Float, leaves as I64 otherwise.
///
/// Scope: only invoked for `Op::GetUpval` PCs the scan classified as
/// `ValueRead` (not the self-recursion call-target marker). The
/// dispatcher pins `JIT_CL` at entry; helper safety relies on that.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_upval_get(idx: i64) -> i64 {
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let cl = unsafe { current_jit_closure() };
    let v = vm.upval_get(cl, idx as u32);
    let (_tag, raw) = v.unpack();
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    unsafe { raw.zero as i64 }
}

/// The trace JIT's typed read of upvalue `idx` of the running closure; see
/// `checked_read`.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_upval_get_checked(idx: i64, want_tag: i64, out: *mut i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    // SAFETY: the trace dispatcher enters with `enter(vm, Some(cl))`, which pins JIT_CL to the running closure.
    let cl = unsafe { current_jit_closure() };
    // SAFETY: see `checked_read`.
    unsafe { checked_read(vm.upval_get(cl, idx as u32), want_tag, out) }
}

/// The method JIT's read of a 5.1/5.2 upvalue that feeds arithmetic: the
/// compiled code takes the payload as a float, the only number type of
/// those dialects. Anything else — nil, a numeric string, a table with
/// `__add` — needs the interpreter, which raises or coerces as the dialect
/// does, so this parks a deopt and the call is re-run there.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_upval_get_float(idx: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let cl = unsafe { current_jit_closure() };
    match vm.upval_get(cl, idx as u32) {
        luna_core::runtime::Value::Float(f) => f.to_bits() as i64,
        _ => {
            vm.jit.pending_err = Some(vm.rt_err("JIT deopt: upvalue is not a float"));
            0
        }
    }
}

/// The LLVM method JIT's check before it reads an upvalue it computes
/// with as an integer. Anything else (a float, nil, a table) needs the
/// interpreter: returns 1 when the upvalue holds an integer, 0 after
/// parking a deopt so the call is re-run there.
// SAFETY: `no_mangle` keeps the symbol resolvable from JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_upval_is_int(idx: i64) -> i64 {
    // SAFETY: called only from JIT-emitted code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    // SAFETY: the method-JIT dispatcher enters with `enter(vm, Some(cl))`, which pins JIT_CL to the running closure.
    let cl = unsafe { current_jit_closure() };
    match vm.upval_get(cl, idx as u32) {
        luna_core::runtime::Value::Int(_) => 1,
        _ => {
            vm.jit.pending_err = Some(vm.rt_err("JIT deopt: upvalue is not an integer"));
            0
        }
    }
}

/// Method-JIT entry check for a chunk compiled with self-recursive
/// calls: they are direct calls to the chunk's own code, which is right
/// only while `upvals[idx]` holds the running closure. A forward-declared
/// local (`local a, b; a = function() ... b() end`) or a reassigned one
/// holds another function, so the call is parked as a deopt and the
/// interpreter runs it. Returns 1 when the upvalue is the running
/// closure, 0 after parking the deopt.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_self_upval_check(idx: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    // SAFETY: the method-JIT dispatcher enters with `enter(vm, Some(cl))`, which pins JIT_CL to the running closure.
    let cl = unsafe { current_jit_closure() };
    match vm.upval_get(cl, idx as u32) {
        luna_core::runtime::Value::Closure(c) if c.ptr_eq(cl) => 1,
        _ => {
            vm.jit.pending_err = Some(vm.rt_err("JIT deopt: callee is not the running closure"));
            0
        }
    }
}

/// The closure the running trace was entered with, as its raw payload
/// bits. A trace inlines a call only while the callee is this closure:
/// the inlined body reads its upvalues through `JIT_CL`.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_head_closure() -> i64 {
    JIT_CL.with(|c| c.get()) as i64
}
