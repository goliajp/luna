//! Deopt parking and guard checks.

use crate::{current_jit_closure, current_jit_vm, str_arg};
use luna_core::__private::VmJitState;
use luna_core::__private::{GcInternals, VmHostInternals, VmJitAlloc};

/// 1 while no deopt is parked and no error is waiting to be raised. A
/// method-JIT chunk checks it after a self-recursive call: once the callee
/// parked one, the caller's result is thrown away and it returns at once
/// instead of computing on.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_no_deopt_parked`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_no_deopt_parked() -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    i64::from(vm.jit().pending_err.is_none() && vm.jit().pending_raise.is_none())
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
    vm.jit_mut().suppress_downrec_admit_once = true;
}

/// 1 when a collection step is due after the allocation compiled code just
/// made (or the call 5.2 checks at), which then leaves after its op for the
/// interpreter to check the op at `op_pc` makes (see
/// `Vm::checkpoint_after_jit`); 0 otherwise.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_gc_due`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_gc_due(op_pc: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    i64::from(vm.jit_gc_due(op_pc as u32))
}

/// Where the count and the point a collection step falls due at lie
/// (`VmJitAlloc::jit_due_words`): compiled code loads it once on entry and
/// compares the two words itself before it calls [`luna_jit_gc_due`].
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_due_words`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_due_words() -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    vm.jit_due_words() as i64
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
    if vm.jit().pending_err.is_none() {
        vm.jit_mut().pending_err =
            Some(vm.runtime_error("JIT deopt: compiled code cannot run this call"));
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
    // SAFETY: `math_key` is an interned string (# Safety)
    let math_key = unsafe { Gc::from_ptr(math_key as *mut LuaStr) };
    let Value::Table(math) = env.get(Value::Str(math_key)) else {
        return 0;
    };
    // SAFETY: `name_key` is an interned string (# Safety)
    let name_key = unsafe { Gc::from_ptr(name_key as *mut LuaStr) };
    let Value::Native(f) = math.get(Value::Str(name_key)) else {
        return 0;
    };
    let lib = match math_key.as_bytes() {
        b"math" => luna_core::__private::math_inlinable_builtin(name_key.as_bytes()),
        b"string" => luna_core::__private::string_inlinable_builtin(name_key.as_bytes()),
        _ => None,
    };
    i64::from(lib.is_some_and(|lib| f.builtin == lib))
}

/// Float `fmod` for a compiled `%`: the interpreter's, which follows the C
/// `fmod` PUC is built with where the binary runs (on x86 Linux an x87
/// `fprem` loop, which picks another NaN than glibc's `fmod` when both
/// operands are NaN).
// SAFETY: no other item in the link is named `luna_jit_fmod`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub extern "C" fn luna_jit_fmod(a: f64, b: f64) -> f64 {
    luna_core::__private::cerrno::fmod(a, b)
}

/// A one-argument C math function for a compiled `math.<fn>(x)`, `f` its
/// number in `MathFn::ALL`, leaving `errno` as the C function does.
// SAFETY: no other item in the link is named `luna_jit_math1`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub extern "C" fn luna_jit_math1(x: f64, f: i64) -> f64 {
    luna_core::__private::cerrno::math1(luna_core::__private::cerrno::MathFn::ALL[f as usize], x)
}

/// For a compiled 5.1 / 5.2 float `for` loop `init, limit, step` (`step` a
/// non-zero integer), with `part` 0: `q = 2^e` such that the loop, counted
/// on integers scaled by `q`, gives the float loop's values bit for bit, 0
/// when no such `e` is known; with `part` 1, the scaled bound
/// `floor(limit / q)` (`ceil` for a negative step), as a double, when there
/// is a `q`.
///
/// `e` is the smaller of the exponent of the lowest set bit of `init` (any,
/// when `init` is ±0) and that of `step`, so `init` and `step` are
/// multiples of `q`, and so is every value `x_k = init + k * step`. The
/// loop stops at the first value past the limit, so every value it reaches
/// has `|x_k| <= max(|init|, |limit| + |step|) <= T = |init| + |limit| +
/// |step|`. `T <= 2^(51 + e)`, with `T` summed in floats, keeps the true
/// `T` below `2^(52 + e)` despite the sum's rounding; then every
/// `|x_k / q|` is below `2^52`: each `x_k` is exact, each float step
/// `x_k + step` is exact, `x_k / q` and its product with `q` are exact, and
/// so is `limit / q` before it is rounded to an integer, so an integer test
/// against that bound agrees with the float test. `e >= -1000` keeps `q`,
/// `1 / q` and every `x_k` normal. A NaN or infinite bound fails the test,
/// as does a subnormal or non-finite `init`.
// SAFETY: no other item in the link is named `luna_jit_for_scale`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub extern "C" fn luna_jit_for_scale(init: f64, limit: f64, step: i64, part: i64) -> f64 {
    match for_scale_parts(init, limit, step) {
        None => 0.0,
        // `q` is a normal double for `|e| <= 1000`, so never 0
        Some(f) if part == 0 => f64::from_bits(((f.e + 1023) as u64) << 52),
        Some(f) => f.bound as f64,
    }
}

/// The counted form of a loop (see [`luna_jit_for_scale`]): `e`, the scaled
/// value one step before `init`, the scaled step, and the scaled bound
/// (`floor(limit / 2^e)`, `ceil` for a negative step).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForScale {
    pub e: i64,
    pub before: i64,
    pub step: i64,
    pub bound: i64,
}

/// The counted form of the loop `init, limit, step`, when its values are
/// all exact (see [`luna_jit_for_scale`]).
pub fn for_scale_parts(init: f64, limit: f64, step: i64) -> Option<ForScale> {
    let e = for_scale(init, limit, step)?;
    // `1 / 2^e`, a normal double for `|e| <= 1000`; every product below is
    // exact and below 2^52 in magnitude
    let inv_q = f64::from_bits(((1023 - e) as u64) << 52);
    let scaled_step = (step as f64 * inv_q) as i64;
    let limit_s = limit * inv_q;
    let bound = if step > 0 {
        limit_s.floor()
    } else {
        limit_s.ceil()
    } as i64;
    Some(ForScale {
        e,
        before: (init * inv_q) as i64 - scaled_step,
        step: scaled_step,
        bound,
    })
}

/// The `e` of [`luna_jit_for_scale`].
fn for_scale(init: f64, limit: f64, step: i64) -> Option<i64> {
    if !init.is_finite() || (init != 0.0 && !init.is_normal()) || step == 0 {
        return None;
    }
    let e_step = i64::from(step.unsigned_abs().trailing_zeros());
    let e = if init == 0.0 {
        e_step
    } else {
        let bits = init.to_bits();
        let exp_field = ((bits >> 52) & 0x7ff) as i64;
        let sig = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
        (exp_field - 1075 + i64::from(sig.trailing_zeros())).min(e_step)
    };
    if e < -1000 {
        return None;
    }
    let total = init.abs() + limit.abs() + step.unsigned_abs() as f64;
    let bound = f64::from_bits(((51 + e + 1023) as u64) << 52);
    // false for a NaN or infinite total
    (total <= bound).then_some(e)
}

/// C `pow` for a compiled `^` of 5.1 to 5.3, leaving `errno` as it does.
// SAFETY: no other item in the link is named `luna_jit_pow`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub extern "C" fn luna_jit_pow(a: f64, b: f64) -> f64 {
    luna_core::__private::cerrno::pow(a, b)
}

/// A compiled `^` of 5.4 and later (`luai_numpow`): `a*a` when `b` is 2,
/// else C `pow`, leaving `errno` as it does.
// SAFETY: no other item in the link is named `luna_jit_numpow`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub extern "C" fn luna_jit_numpow(a: f64, b: f64) -> f64 {
    if b == 2.0 {
        a * a
    } else {
        luna_core::__private::cerrno::pow(a, b)
    }
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
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call, and `s` is
    // a live string
    let (vm, s) = unsafe { (current_jit_vm(), str_arg(s)) };
    // 0 with a deopt parked when the string cannot be made, as the
    // allocating helpers do
    luna_core::__private::str_sub(vm, s, i, j).map_or(0, |r| r.as_ptr() as i64)
}

/// [`luna_jit_str_sub`] for 5.1 and 5.2, which check for a collection
/// before the string is made: 0 without a deopt when one is due.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `s` is a live string.
// SAFETY: no other item in the link is named `luna_jit_str_sub_checked`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_sub_checked(s: i64, i: i64, j: i64) -> i64 {
    // SAFETY: as for `luna_jit_str_sub`
    let (vm, s) = unsafe { (current_jit_vm(), str_arg(s)) };
    luna_core::__private::str_sub_checked(vm, s, i, j).map_or(0, |r| r.as_ptr() as i64)
}
