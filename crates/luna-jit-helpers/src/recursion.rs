//! Self-recursive method-JIT calls: the levels they make, counted as PUC
//! counts its calls, and the calls made in the interpreter when the native
//! stack runs low.
//!
//! A compiled self-recursive call is a native call, so deep recursion uses
//! native stack and makes no interpreter frame. Each level carries its
//! frame's base and its depth; a level that reaches the base or depth the
//! context allows calls [`luna_jit_self_level`], which counts it (the
//! stack its frame needs, its `CallInfo`, 5.2's collection check) and
//! sets the next limits. The compiled code checks the stack pointer once
//! every so many levels; when the stack runs low,
//! [`luna_jit_self_call_slow`] makes the call in the interpreter instead.
//!
//! Both tiers keep the limits and the failure flag in a context their
//! entry fills ([`luna_jit_enter_ctx`]): the Cranelift tier holds its
//! address in the pinned register, the LLVM tier passes it.

use luna_core::runtime::Value;

use crate::{current_jit_closure, current_jit_vm, payload_bits, table_arg};

/// The kind of a self-recursive call's result, in a [`self_call_desc`].
pub const SELF_CALL_RET_NONE: i64 = 0;
/// See [`SELF_CALL_RET_NONE`].
pub const SELF_CALL_RET_INT: i64 = 1;
/// See [`SELF_CALL_RET_NONE`].
pub const SELF_CALL_RET_FLOAT: i64 = 2;
/// See [`SELF_CALL_RET_NONE`].
pub const SELF_CALL_RET_TABLE: i64 = 3;

/// Words of a self-call context: the stack address below which a self
/// call goes through [`luna_jit_self_call_slow`], the flag a failed call
/// sets, the base and the depth from which a level calls
/// [`luna_jit_self_level`], and the base of level 0.
pub const SELF_CTX_WORDS: usize = 5;

/// What [`luna_jit_self_call_slow`] needs to know about the call, packed
/// into the constant compiled code passes it: the argument count, which
/// arguments are floats and which tables, and the result's kind.
pub fn self_call_desc(nargs: u32, float_mask: u8, table_mask: u8, ret: i64) -> i64 {
    i64::from(nargs & 0xff)
        | (i64::from(float_mask) << 8)
        | (i64::from(table_mask) << 16)
        | (ret << 24)
}

/// Set in the desc [`luna_jit_self_level`] gets for the callee of a self
/// tail call.
pub const SELF_LEVEL_TAIL: i64 = 1 << 40;

/// A desc whose levels hold no table: the collector needs nothing of them.
fn bare(desc: i64) -> bool {
    (desc >> 16) & 0xff == 0
}

/// Fill a self-call context for level 0 of the running closure: the stack
/// address below which a self call goes through
/// [`luna_jit_self_call_slow`] (0 when the thread's stack bounds are not
/// known), the failure flag cleared, the limits of the levels after it and
/// its base.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread
/// opened with the running closure; `ctx` points to [`SELF_CTX_WORDS`]
/// writable words.
// SAFETY: no other item in the link is named `luna_jit_enter_ctx`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_enter_ctx(ctx: *mut i64) {
    // SAFETY: inside an enter_jit window opened with the running closure (# Safety) JIT_VM is the
    // Vm lent to this call and JIT_CL that closure
    let (vm, cl) = unsafe { (current_jit_vm(), current_jit_closure()) };
    let lim = vm.jit_limits(cl, 0);
    // SAFETY: `ctx` points to SELF_CTX_WORDS writable words (# Safety)
    unsafe {
        *ctx = luna_core::native_stack::jit_limit() as i64;
        *ctx.add(1) = 0;
        *ctx.add(2) = i64::from(lim.base);
        *ctx.add(3) = i64::from(lim.depth);
        *ctx.add(4) = i64::from(vm.jit.levels_slot);
    }
}

/// Count level `depth` of the running closure's self recursion, its frame
/// at `base` (see [`luna_core::vm::Vm::jit_self_level`]), and set the
/// context's limits for the levels after it. 0 when the level may go on;
/// otherwise the error is left in `vm.jit.pending_raise`, the failure flag
/// set, and the level returns at once.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread
/// opened with the running closure; `ctx` points to [`SELF_CTX_WORDS`]
/// writable words.
// SAFETY: no other item in the link is named `luna_jit_self_level`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_self_level(
    ctx: *mut i64,
    desc: i64,
    base: i64,
    depth: i64,
) -> i64 {
    // SAFETY: inside an enter_jit window opened with the running closure (# Safety) JIT_VM is the
    // Vm lent to this call and JIT_CL that closure
    let (vm, cl) = unsafe { (current_jit_vm(), current_jit_closure()) };
    // a call below already failed: the callers are only unwinding
    let r = if vm.jit.pending_raise.is_some() || vm.jit.pending_err.is_some() {
        None
    } else {
        let tail = desc & SELF_LEVEL_TAIL != 0;
        match vm.jit_self_level(cl, base as u32, depth as u32, bare(desc), tail) {
            Ok(lim) => Some(lim),
            Err(e) => {
                vm.jit.pending_raise = Some(e);
                None
            }
        }
    };
    // SAFETY: `ctx` points to SELF_CTX_WORDS writable words (# Safety)
    unsafe {
        match r {
            Some(lim) => {
                *ctx.add(2) = i64::from(lim.base);
                *ctx.add(3) = i64::from(lim.depth);
                0
            }
            None => {
                *ctx.add(1) = 1;
                1
            }
        }
    }
}

/// Make the running closure's self-recursive call at level `depth`, its
/// frame at `base`, that compiled code could not make natively, with the
/// arguments `a0..` described by `desc` ([`self_call_desc`]), and return
/// its result as compiled code holds it. `ctx` is the code's context.
/// When the call fails, the error is left in `vm.jit.pending_raise` for
/// the dispatcher to raise; when its result is not of the kind the
/// compiled code expects, a deopt is parked. Either way the context's
/// failure flag is set: callers whose going on could be seen return at
/// once, the others finish with dummy results that the dispatcher drops,
/// and every self call they still make returns at once.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread
/// opened with the running closure; the table arguments `desc` names are
/// pointers of live tables; `ctx` points to [`SELF_CTX_WORDS`] writable
/// words.
// SAFETY: no other item in the link is named `luna_jit_self_call_slow`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn luna_jit_self_call_slow(
    ctx: *mut i64,
    desc: i64,
    base: i64,
    depth: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
) -> i64 {
    // SAFETY: inside an enter_jit window opened with the running closure (# Safety) JIT_VM is the
    // Vm lent to this call and JIT_CL that closure
    let (vm, cl) = unsafe { (current_jit_vm(), current_jit_closure()) };
    // a call below already failed: the callers are only unwinding
    if vm.jit.pending_raise.is_some() || vm.jit.pending_err.is_some() {
        // SAFETY: `ctx` points to SELF_CTX_WORDS writable words (# Safety)
        unsafe { *ctx.add(1) = 1 };
        return 0;
    }
    let nargs = (desc & 0xff) as usize;
    let float_mask = (desc >> 8) & 0xff;
    let table_mask = (desc >> 16) & 0xff;
    let ret = desc >> 24;
    let raw = [a0, a1, a2, a3];
    let mut args = [Value::Nil; 4];
    for i in 0..nargs.min(4) {
        args[i] = if (table_mask >> i) & 1 == 1 {
            // SAFETY: a table argument is the pointer of a live table (# Safety)
            Value::Table(unsafe { table_arg(raw[i]) })
        } else if (float_mask >> i) & 1 == 1 {
            Value::Float(f64::from_bits(raw[i] as u64))
        } else {
            Value::Int(raw[i])
        };
    }
    let int_as_float = vm.version() <= luna_core::version::LuaVersion::Lua52;
    let result = vm.jit_call_interpreted(cl, &args[..nargs], base as u32, depth as u32, bare(desc));
    let out = match result {
        Ok(vals) => {
            let v = vals.first().copied().unwrap_or(Value::Nil);
            match (ret, v) {
                (SELF_CALL_RET_NONE, _) => Some(0),
                (SELF_CALL_RET_INT, Value::Int(i)) => Some(i),
                (SELF_CALL_RET_FLOAT, Value::Float(f)) => Some(f.to_bits() as i64),
                (SELF_CALL_RET_FLOAT, Value::Int(i)) if int_as_float => {
                    Some((i as f64).to_bits() as i64)
                }
                (SELF_CALL_RET_TABLE, Value::Table(_)) => Some(payload_bits(v)),
                _ => {
                    vm.jit.pending_err =
                        Some(vm.rt_err("JIT deopt: interpreted result of another kind"));
                    None
                }
            }
        }
        Err(e) => {
            vm.jit.pending_raise = Some(e);
            None
        }
    };
    match out {
        Some(bits) => bits,
        None => {
            // SAFETY: `ctx` points to SELF_CTX_WORDS writable words (# Safety)
            unsafe { *ctx.add(1) = 1 };
            0
        }
    }
}
