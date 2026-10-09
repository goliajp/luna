//! Self-recursive method-JIT calls when the native stack runs low, or when
//! the dialect's call-depth limit (5.1's `LUAI_MAXCALLS`) is reached.
//!
//! A compiled self-recursive call is a native call, so deep recursion uses
//! native stack and makes no interpreter frame. The compiled code checks
//! the stack pointer against a limit and its remaining call budget once
//! every so many levels; when either runs out, [`luna_jit_self_call_slow`]
//! makes the call instead: in the interpreter, which goes on to any depth
//! the Lua stack allows and fails with Lua's own "stack overflow", or, with
//! no budget left, by raising that error itself.
//!
//! Both tiers keep limit, failure flag and budget in a context their entry
//! fills ([`luna_jit_enter_ctx`]): the Cranelift tier holds its address in
//! the pinned register, the LLVM tier passes it, the limit and the calls
//! left as arguments of its self calls.

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

/// Words of a Cranelift self-call context: the stack limit, the flag a
/// failed call sets, the calls left, and the calls the budget started
/// with.
pub const SELF_CTX_WORDS: usize = 4;

/// The `left` [`luna_jit_self_call_slow`] gets from code that does not
/// count its calls: the budget is then the thread's own.
pub const SELF_CALL_UNCOUNTED: i64 = i64::MIN;

/// What [`luna_jit_self_call_slow`] needs to know about the call, packed
/// into the constant compiled code passes it: the argument count, which
/// arguments are floats and which tables, and the result's kind.
pub fn self_call_desc(nargs: u32, float_mask: u8, table_mask: u8, ret: i64) -> i64 {
    i64::from(nargs & 0xff)
        | (i64::from(float_mask) << 8)
        | (i64::from(table_mask) << 16)
        | (ret << 24)
}

/// Fill a Cranelift self-call context: the stack address below which a
/// self call goes through [`luna_jit_self_call_slow`] (0 when the thread's
/// stack bounds are not known), the failure flag cleared, and the calls
/// the running thread may still nest.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread;
/// `ctx` points to [`SELF_CTX_WORDS`] writable words.
// SAFETY: no other item in the link is named `luna_jit_enter_ctx`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_enter_ctx(ctx: *mut i64) {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    let budget = vm.jit_call_budget(0);
    // SAFETY: `ctx` points to SELF_CTX_WORDS writable words (# Safety)
    unsafe {
        *ctx = luna_core::native_stack::jit_limit() as i64;
        *ctx.add(1) = 0;
        *ctx.add(2) = budget;
        *ctx.add(3) = budget;
    }
}

/// Make the running closure's self-recursive call that compiled code could
/// not make natively, with the arguments `a0..` described by `desc`
/// ([`self_call_desc`]), and return its result as compiled code holds it.
/// `ctx` is the code's context;
/// `left` the calls the code had left, or [`SELF_CALL_UNCOUNTED`].
/// With no calls left in the budget the call raises "stack overflow";
/// otherwise the interpreter makes it. When the call fails, the error is
/// left in `vm.jit.pending_raise` for the dispatcher to raise; when its
/// result is not of the kind the compiled code expects, a deopt is parked.
/// Either way the context's failure flag is set: callers whose going on
/// could be seen
/// return at once, the others finish with dummy results that the
/// dispatcher drops, and every self call they still make lands here and
/// returns at once.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread
/// opened with the running closure; the table arguments `desc` names are
/// pointers of live tables; `ctx` is null or points to [`SELF_CTX_WORDS`]
/// writable words.
// SAFETY: no other item in the link is named `luna_jit_self_call_slow`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_self_call_slow(
    ctx: *mut i64,
    desc: i64,
    left: i64,
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
        if !ctx.is_null() {
            // SAFETY: a non-null `ctx` points to SELF_CTX_WORDS writable words (# Safety)
            unsafe { *ctx.add(1) = 1 };
        }
        return 0;
    }
    let budget = if left == SELF_CALL_UNCOUNTED {
        vm.jit_call_budget(0)
    } else {
        left
    };
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
    let result = if budget <= 0 {
        Err(vm.jit_depth_error(cl))
    } else {
        // the compiled calls below count against 5.1's call limit
        let native = (left != SELF_CALL_UNCOUNTED && !ctx.is_null()).then(|| {
            // SAFETY: a non-null `ctx` points to SELF_CTX_WORDS words (# Safety)
            let initial = unsafe { *ctx.add(3) };
            (initial - left).clamp(0, i64::from(u32::MAX)) as u32
        });
        vm.jit_call_interpreted(cl, &args[..nargs], native)
    };
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
            if !ctx.is_null() {
                // SAFETY: a non-null `ctx` points to SELF_CTX_WORDS writable words (# Safety)
                unsafe { *ctx.add(1) = 1 };
            }
            0
        }
    }
}
