//! Self-recursive method-JIT calls: the levels they make, counted as PUC
//! counts its calls, and the calls made in the interpreter when the native
//! stack runs low.
//!
//! A compiled self-recursive call is a native call, so deep recursion uses
//! native stack and makes no interpreter frame. Each level carries its
//! frame's base and its depth; a level that reaches the base or depth the
//! context allows calls [`luna_jit_self_level`], which counts it (the
//! stack its frame needs, its `CallInfo`, 5.2's collection check) and
//! sets the next limits. When the call's check would find a collection
//! step due, or the native stack runs low (the stub checks the stack
//! pointer once every so many levels), the levels go to the interpreter
//! instead ([`luna_jit_handoff_frame`]), which makes the call.
//!
//! Both tiers keep the limits and the failure flag in a context their
//! entry fills ([`luna_jit_enter_ctx`]): the Cranelift tier holds its
//! address in the pinned register, the LLVM tier passes it.

use luna_core::runtime::Value;

use crate::{current_jit_closure, current_jit_vm, table_arg};

/// The kind of a self-recursive call's result, in a [`self_call_desc`].
pub const SELF_CALL_RET_NONE: i64 = 0;
/// See [`SELF_CALL_RET_NONE`].
pub const SELF_CALL_RET_INT: i64 = 1;
/// See [`SELF_CALL_RET_NONE`].
pub const SELF_CALL_RET_FLOAT: i64 = 2;
/// See [`SELF_CALL_RET_NONE`].
pub const SELF_CALL_RET_TABLE: i64 = 3;

/// Words of a self-call context: the stack address below which a self
/// call goes to the interpreter, the flag a level returning early sets
/// ([`SELF_FAILED`], [`SELF_HANDED`], [`SELF_NOT_STARTED`]), the base from
/// which a level calls [`luna_jit_self_level`], and the base of level 0.
pub const SELF_CTX_WORDS: usize = 4;

/// What [`luna_jit_self_call_slow`] needs to know about the call, packed
/// into the constant compiled code passes it: the argument count, which
/// arguments are floats and which tables, and the result's kind.
pub fn self_call_desc(nargs: u32, float_mask: u8, table_mask: u8, ret: i64) -> i64 {
    i64::from(nargs & 0xff)
        | (i64::from(float_mask) << 8)
        | (i64::from(table_mask) << 16)
        | (ret << 24)
}

/// The context's flag: a call below failed (its error or deopt parked).
pub const SELF_FAILED: i64 = 1;
/// The context's flag: the level above handed its frame to the
/// interpreter, which goes on with it after this call.
pub const SELF_HANDED: i64 = 2;
/// The context's flag: the call did not start; the interpreter makes it.
pub const SELF_NOT_STARTED: i64 = 3;

/// Set in the desc [`luna_jit_self_level`] gets for the callee of a self
/// tail call.
pub const SELF_LEVEL_TAIL: i64 = 1 << 40;

/// Fill a self-call context for level 0 of the running closure: the stack
/// address below which a self call goes to the interpreter (0 when the
/// thread's stack bounds are not known), the failure flag cleared, the limits of the levels after it and
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
    let base = vm.jit.levels_slot;
    let lim = vm.jit_limits(cl, base);
    // SAFETY: `ctx` points to SELF_CTX_WORDS writable words (# Safety)
    unsafe {
        *ctx = luna_core::native_stack::jit_limit() as i64;
        *ctx.add(1) = 0;
        *ctx.add(2) = i64::from(lim.base);
        *ctx.add(3) = i64::from(base);
    }
}

/// Count level `depth` of the running closure's self recursion, its frame
/// at `base` (see [`luna_core::vm::Vm::jit_self_level`]), and set the
/// context's limits for the levels after it. 0 when the level may go on;
/// otherwise the level returns at once, the flag set: [`SELF_FAILED`]
/// with the error left in `vm.jit.pending_raise`, or [`SELF_NOT_STARTED`]
/// when the interpreter is to make the call.
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
        Err(SELF_FAILED)
    } else {
        let tail = desc & SELF_LEVEL_TAIL != 0;
        match vm.jit_self_level(cl, base as u32, depth as u32, tail) {
            Ok(Some(lim)) => Ok(lim),
            Ok(None) => Err(SELF_NOT_STARTED),
            Err(e) => {
                vm.jit.pending_raise = Some(e);
                Err(SELF_FAILED)
            }
        }
    };
    // SAFETY: `ctx` points to SELF_CTX_WORDS writable words (# Safety)
    unsafe {
        match r {
            Ok(lim) => {
                *ctx.add(2) = i64::from(lim.base);
                0
            }
            Err(flag) => {
                *ctx.add(1) = flag;
                1
            }
        }
    }
}

/// Record the frame of a compiled level that hands itself to the
/// interpreter (see [`luna_core::vm::Vm::jit_handoff_frame`]): the
/// interpreter goes on with it at `pc`, its frame at `base` (level 0's
/// when negative), its registers the `n` pairs of tag and payload words
/// at `regs`.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread;
/// `regs` points at `2 * n` readable words, each pair one register's tag
/// and payload.
// SAFETY: no other item in the link is named `luna_jit_handoff_frame`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_handoff_frame(pc: i64, base: i64, regs: i64, n: i64) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    // SAFETY: `regs` points at `2 * n` readable words (# Safety)
    let words = unsafe { std::slice::from_raw_parts(regs as *const i64, 2 * n as usize) };
    // before 5.3 every number is a float
    let float_only = vm.version() <= luna_core::version::LuaVersion::Lua52;
    let vals = words
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[tag, payload]| reg_value(tag, payload, float_only))
        .collect();
    let base = u32::try_from(base).ok();
    vm.jit_handoff_frame(base, pc as u32, vals);
}

/// A register compiled code holds, by the tag its kind gives it: a table
/// register holding 0 is nil.
fn reg_value(tag: i64, payload: i64, float_only: bool) -> Value {
    use luna_core::runtime::value::raw;
    match tag as u8 {
        raw::INT if float_only => Value::Float(payload as f64),
        raw::INT => Value::Int(payload),
        raw::FLOAT => Value::Float(f64::from_bits(payload as u64)),
        // SAFETY: a non-zero table register holds a live table
        raw::TABLE if payload != 0 => Value::Table(unsafe { table_arg(payload) }),
        _ => Value::Nil,
    }
}
