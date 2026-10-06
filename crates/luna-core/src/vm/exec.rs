//! The interpreter. Dispatch is a plain match over opcodes. Lua→Lua calls
//! share one loop and never recurse the Rust stack; only native↔Lua boundaries do (e.g. pcall).
//!
//! Varargs follow 5.5 semantics: a vararg call materializes a vararg table
//! (fields 1..n plus "n") kept in the function's own stack slot; `...`
//! expands from it and `...name` binds it. 5.1 LUAI_COMPAT_VARARG also
//! materializes a local `arg` table (see `proto.has_compat_vararg_arg`).

use crate::frontend::SyntaxError;
use crate::jit::send_compat::TArc;
use crate::native_stack;
use crate::numeric::{self, Num};
use crate::runtime::{
    AfterClose, CallFrame, CloseCont, ContKind, Coro, CoroStatus, Frame, Gc, Heap, HostCont,
    LuaClosure, MetaAction, MetaCont, NativeCont, Table, TableError, UpvalState, Upvalue, Value,
};
use crate::version::LuaVersion;
use crate::vm::callstack::DbgKind;
use crate::vm::error::LuaError;
use crate::vm::isa::{Inst, Op};
use native_call::NativeKind;

mod arith;
mod call;
mod call_fast;
mod close;
mod compare;
#[cfg(test)]
mod cont_trap_tests;
mod coro;
mod coro_resume;
mod dispatch;
mod errors;
mod fast;
mod fast_arith;
mod finalize;
mod for_loop;
mod frame_ops;
mod frame_state;
mod frames_sync;
use frames_sync::{frames_pop_known, frames_pop_sync, frames_push_sync};
mod gc;
mod hooks;
mod host_api;
pub mod host_c;
mod index;
mod index_fast;
mod index_miss;
mod index_set;
mod jit_call;
mod jit_rt;
mod lifecycle;
mod limits;
mod load;
mod meta;
mod names;
mod native_args;
pub(crate) mod native_call;
mod num;
mod num_double;
mod protected;
mod settings;
mod slow_ops;
mod state;
mod strings;
mod trace_adopt;
mod trace_cache;
mod trace_close;
mod trace_dispatch;
mod trace_exit;
mod trace_exit_decode;
mod trace_exit_inner;
mod trace_record;
mod trace_record_slots;
mod trace_start;
mod trace_stats;
mod trace_wire;
mod unwind;
use coro_resume::*;
pub use errors::SpecialErrors;
pub(crate) use hooks::HOOK_YIELD_SLOT;
pub use hooks::{
    HOOK_MASK_CALL, HOOK_MASK_COUNT, HOOK_MASK_LINE, HOOK_MASK_RETURN, HookState, RustDebugHook,
    RustHookEvent,
};
pub use load::{ParsedText, TextLoad};
pub(crate) use meta::Mm;
use meta::*;
use num::*;
pub(crate) use num::{ArithOp, arith_num, c_fmod, str_to_num};
pub(crate) use state::AsyncNativeCallCtx;
pub use state::Vm;
use trace_cache::*;
use trace_exit_decode::{ExitSource, keep_tfor_vars};
use trace_wire::hold_side_trace;
use unwind::*;

/// PUC MAXTAGLOOP (5.3+): bound on `__index`/`__newindex` chains.
const MAX_TAG_LOOP: u32 = 2000;
/// PUC `MAXCCMT`: bound on a `__call` metamethod chain (lvm.c). 200 chains
/// is more than any reasonable program needs and matches PUC 5.4/5.5; a
/// bound of `15` is tight enough to fire on calls.lua :194 (N=20).
const MAX_CCMT: u32 = 200;
/// PUC `LUAI_MAXCCALLS`: the C level a call may not run at. luna counts
/// what PUC's `nCcalls` counts: calls made from native code (`c_depth`)
/// and the calls the interpreter makes through a continuation frame, which
/// PUC makes through its C stack (`pcall_depth`: pcall / xpcall,
/// metamethods, `__pairs`, `__close`).
pub(crate) const MAX_C_DEPTH: u32 = 200;
/// The level at which a call made while a message handler runs fails with
/// "error in error handling": PUC 5.4's `luaE_checkcstack`
/// (`LUAI_MAXCCALLS / 10 * 11`) and 5.1–5.3's `luaD_call`
/// (`LUAI_MAXCCALLS + LUAI_MAXCCALLS >> 3`).
const ERRERR_C_DEPTH: u32 = 220;
const ERRERR_C_DEPTH_PRE54: u32 = 225;
/// PUC 5.1 `LUAI_MAXCALLS`: nested Lua calls a 5.1 thread may make.
const MAX_CALLS_51: u32 = 20000;
/// Stack an xpcall handler may use past `MAX_LUA_STACK` while handling a
/// stack overflow: PUC's 200 extra `ERRORSTACKSIZE` slots, plus the frame
/// reserve the overflowing call was refused, so the handler's first frame
/// fits where that one did not (PUC `ERRORSTACKSIZE - LUAI_MAXSTACK`).
const STACK_ERR_SPACE: u32 = 200;
/// Where a thread's stack ends for a call needing `n` slots at top `t`
/// (both counted from the first function, as luna's slots are; PUC's
/// slot 0 holds no function, so its `L->top - L->stack` is one more):
/// the call fails when `t + n` passes it. PUC 5.4 and 5.5 fail when
/// `L->top - L->stack + n > LUAI_MAXSTACK`, 5.2 and 5.3 when that sum
/// plus `EXTRA_STACK` (5) does. 5.1 has no stack limit: its 20000-frame
/// `LUAI_MAXCALLS` ends a recursion first (`MAX_CALLS_51`), and the cap
/// here only bounds memory.
/// PUC 5.1's `BASIC_CI_SIZE`: the frames a thread's `CallInfo` array
/// holds at first.
const BASIC_FRAME_SIZE_51: u32 = 8;

/// The lowest `lua_stack_limit` of any dialect: below it a call needs no
/// exact check.
const STACK_LIMIT_FLOOR: u32 = 1_000_000 - 5 - 1;

fn lua_stack_limit(version: LuaVersion) -> u32 {
    match version {
        LuaVersion::Lua51 => (1 << 20) - 1,
        LuaVersion::Lua52 | LuaVersion::Lua53 => 1_000_000 - 5 - 1,
        _ => 1_000_000 - 1,
    }
}
/// PUC `LUAI_MAXSTACK` (`luaconf.h`): the cap library code consults via
/// `lua_checkstack` to refuse multi-value pushes (`table.unpack` returning
/// N values, `string.pack` results, etc.). 5.3 coroutine.lua :530 pins
/// this at one million — `for j in {lim-10, …}` expects every j ≥ lim-10
/// to fail because the few slots already consumed in the coroutine push
/// the effective cap below lim-10.
const PUC_MAXSTACK: i64 = 1_000_000;

/// PUC 5.4+ default warnf state. The base library's `warn` function flips
/// between `Off` and `On` via the `@on` / `@off` control messages; any other
/// `@<word>` control is silently ignored, mirroring `lauxlib.c::checkcontrol`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WarnState {
    /// `warn` calls are silently dropped (default after `warn("@off")`).
    Off,
    /// `warn` calls are delivered to stderr (after `warn("@on")`).
    On,
}

/// Best-effort extraction of a textual message from a `catch_unwind` payload.
/// `panic!("msg")` arrives as `String`, `panic!(static)` as `&str`; anything
/// else degrades to `"<non-string panic>"`. Used by the native-call
/// catch_unwind to fold the panic into a Lua error.
fn panic_payload_str(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    "<non-string panic>".to_string()
}

/// Combined error type returned by [`Vm::eval`] and friends — either the
/// chunk failed to parse / compile, or it raised at runtime.
#[derive(Debug)]
pub enum Error {
    /// Parse or compile failure.
    Syntax(SyntaxError),
    /// Runtime error raised during execution.
    Runtime(LuaError),
}

impl From<SyntaxError> for Error {
    fn from(e: SyntaxError) -> Error {
        Error::Syntax(e)
    }
}

impl From<LuaError> for Error {
    fn from(e: LuaError) -> Error {
        Error::Runtime(e)
    }
}

/// One-time env-var read for
/// `LUNA_AOT_PROBE`. Returns `true` iff the env var is set to any
/// non-empty value. The result is cached in a `OnceLock` so the
/// dispatcher's hot path pays a single atomic load per process. Off
/// by default — production deploys don't bleed diagnostic prints.
fn jit_probe_enabled() -> bool {
    static PROBE_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE_ON.get_or_init(|| {
        std::env::var("LUNA_AOT_PROBE")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
    })
}
