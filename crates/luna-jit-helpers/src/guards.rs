//! Deopt parking and guard checks.

use crate::{current_jit_closure, current_jit_vm};

/// 1 while no deopt is parked. A method-JIT chunk checks it after a
/// self-recursive call: once the callee parked one, the caller's result
/// is thrown away and it returns at once instead of computing on.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_no_deopt_parked`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_no_deopt_parked() -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    i64::from(vm.jit.pending_err.is_none())
}

/// A trace side exit that resumes at the trace's own head: the op there
/// has not run, so the dispatcher must let the interpreter run it before
/// admitting the trace again, or the two would hand the same pc back and
/// forth forever.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_suppress_trace_admit`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_suppress_trace_admit() {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit.suppress_downrec_admit_once = true;
}

/// Parks a deopt for the running method-JIT call, which then returns at
/// once: the dispatcher discards its result and runs the call in the
/// interpreter. Used where compiled code finds, before doing anything
/// observable, that it cannot compute the result.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_park_deopt`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_park_deopt() {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_none() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: compiled code cannot run this call"));
    }
}

/// Whether `_ENV.<lib>.<name>` of the running closure (`lib` being `math`
/// or `string`) is still the library function the JIT inlined for
/// `<lib>.<name>(...)`: 1 if it is, 0 otherwise. Raw reads suffice: a
/// field that is present is found before any `__index`, and an absent
/// one is not the library function.
/// The keys are interned strings the compiled code baked in; `_ENV` is
/// upvalue 0, as the fold matchers require.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread opened with the running
/// closure; `math_key` and `name_key` are interned strings.
// SAFETY: no other item in the link is named `luna_jit_math_fn_is_library`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_math_fn_is_library(math_key: i64, name_key: i64) -> i64 {
    use luna_core::runtime::{Gc, LuaStr, Value};
    // SAFETY: inside an enter_jit window opened with the running closure (# Safety) JIT_VM is the
    // Vm lent to this call and JIT_CL that closure
    let (vm, cl) = unsafe { (current_jit_vm(), current_jit_closure()) };
    let Value::Table(env) = vm.upval_get(cl, 0) else {
        return 0;
    };
    let math_key = Gc::from_ptr(math_key as *mut LuaStr);
    let Value::Table(math) = env.get(Value::Str(math_key)) else {
        return 0;
    };
    let name_key = Gc::from_ptr(name_key as *mut LuaStr);
    let Value::Native(f) = math.get(Value::Str(name_key)) else {
        return 0;
    };
    let lib = match math_key.as_bytes() {
        b"math" => luna_core::vm::lib_math::inlinable_native(name_key.as_bytes()),
        b"string" => luna_core::vm::lib_string::inlinable_native(name_key.as_bytes()),
        _ => None,
    };
    i64::from(lib.is_some_and(|lib| std::ptr::fn_addr_eq(f.f, lib)))
}

/// `string.sub(s, i, j)` for the trace JIT, which checked that the call is
/// the library function and its arguments a string and two integers (`j`
/// -1 when the call gave none). Returns the result string.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `s` is a live string.
// SAFETY: no other item in the link is named `luna_jit_str_sub`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_sub(s: i64, i: i64, j: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    let s = luna_core::runtime::Gc::from_ptr(s as *mut luna_core::runtime::LuaStr);
    luna_core::vm::lib_string::str_sub(vm, s, i, j).as_ptr() as i64
}
