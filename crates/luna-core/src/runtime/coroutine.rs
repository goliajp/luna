//! Coroutine (thread) objects. A coroutine owns a full execution context
//! — value stack, call frames, open upvalues, to-be-closed slots and stack top
//! — that is swapped into the running `Vm` while it is active and saved back
//! here while it is suspended.

use crate::runtime::Upvalue;
use crate::runtime::dialect::Dialect;
use crate::runtime::function::{CallFrame, ContKind};
use crate::runtime::heap::{Gc, GcHeader, Marker};
use crate::runtime::mem::LVec;
use crate::runtime::table::Table;
use crate::runtime::value::Value;

/// Lua coroutine status (PUC `coroutine.status`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoroStatus {
    /// created or yielded — resumable
    Suspended,
    /// currently executing (the running thread)
    Running,
    /// resumed another coroutine and is waiting for it
    Normal,
    /// finished or errored — not resumable
    Dead,
}

/// How a thread that died by an error failed, which the C API reports as
/// its status (PUC keeps the error status in `L->status`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ErrorStatus {
    /// any other error (`LUA_ERRRUN`)
    #[default]
    Run,
    /// a memory error (`LUA_ERRMEM`)
    Mem,
    /// a message handler failed (`LUA_ERRERR`)
    Err,
    /// a finalizer failed in a 5.2/5.3 collection (`LUA_ERRGCMM`)
    Gcmm,
}

/// A Lua coroutine (`thread`) — one independent execution context plus its
/// saved value/frame stacks and resume linkage.
#[repr(C)]
pub struct Coro {
    pub(crate) hdr: GcHeader,
    /// Resume state (suspended / running / normal / dead).
    pub status: CoroStatus,
    /// the body function, kept for the first resume (and as a GC root)
    pub body: Value,
    /// whether the body frame has been pushed yet (first resume vs. continue)
    pub started: bool,
    /// the coroutine that resumed this one (to restore on yield/return and for
    /// `coroutine.running`); `None` once suspended/dead
    pub resumer: Option<Gc<Coro>>,
    /// where execution suspended on `yield`: the call slot and result count to
    /// finish that call with the next resume's arguments
    pub resume_at: Option<(u32, i32)>,
    /// the error object a coroutine died with (when it errored rather than
    /// returned); `coroutine.close` reports it once, then clears it
    pub error_value: Option<Value>,
    /// the status of the error it died with
    pub error_status: ErrorStatus,
    /// the frames and the natives running in it when an error killed it, as
    /// PUC leaves a dead thread's stack in place: what `debug.traceback`,
    /// `debug.getinfo` and `Vm::coroutine_error_traceback` see of it
    pub(crate) dead: Option<crate::vm::callstack::DeadStack>,
    /// while it waits on a coroutine it resumed (status normal): its own
    /// running natives, as indices into the Vm's `running_natives`
    pub(crate) natives: std::ops::Range<usize>,
    // ---- saved execution context (valid while suspended/normal) ----
    /// Saved value stack.
    pub stack: LVec<Value>,
    /// Saved frame stack (Lua frames + native continuations).
    pub frames: LVec<CallFrame>,
    /// Open-upvalue list — `(stack slot, upvalue cell)` pairs.
    pub open_upvals: LVec<(u32, Gc<Upvalue>)>,
    /// Stack indices of registered `<close>` slots (5.4+).
    pub tbc: LVec<u32>,
    /// Saved stack top.
    pub top: u32,
    /// The top of the native that switched the thread away (PUC's top of a
    /// suspended thread); `u32::MAX` (the whole stack) before it first runs
    /// or when the host switched it.
    pub(crate) gc_top: u32,
    /// Its top when it last yielded from `coroutine.yield`, the yielded
    /// values gone to the resumer (PUC `lua_xmove`); `u32::MAX` after a
    /// yield from a C function of the host.
    pub(crate) yield_top: u32,
    /// Saved `Vm::stack_size`: PUC's size of the thread's stack, 0 for a
    /// thread that has not run.
    pub stack_size: u32,
    /// Saved `Vm::frame_size`: the size of the thread's 5.1 frame array.
    pub frame_size: u32,
    /// Saved `Vm::ci_alloc`: PUC 5.2+'s `CallInfo`s of the thread.
    pub(crate) ci_alloc: u32,
    /// Saved `Vm::meta_conts`.
    /// live pcall/xpcall continuation count (PUC nCcalls portion); see Vm
    pub meta_conts: u32,
    /// Saved `Vm::host_conts`.
    pub(crate) host_conts: u32,
    /// Saved `Vm::stale_frames`.
    pub stale_frames: u32,
    /// suspended by a C function's yield: that function's `ci->top`
    pub(crate) yield_ci_top: u32,
    /// this thread's debug hook state (PUC per-thread hook/hookmask)
    pub hook: crate::vm::exec::HookState,
    /// PUC `L->l_gt` — the thread's own globals table. Captured from the
    /// resuming thread at create time, then swapped with `Vm.globals` on
    /// every resume/yield boundary so a `setfenv(0, env)` inside the
    /// coroutine only retunes *this* thread (5.1 closure.lua :177 pins
    /// this — yielding `getfenv()` after the rewire must see the
    /// coroutine's own per-closure env, not the caller's).
    pub globals: Gc<Table>,
    /// The thread's stack as the C API sees it (PUC `lua_State`'s stack):
    /// what `lua_push*` pushes on this thread and where C functions running
    /// on it find their arguments.
    #[doc(hidden)]
    pub host_stack: LVec<Value>,
    /// The C API's `lua_State` for this thread, made the first time C asks
    /// for one; it lives as long as the thread.
    #[doc(hidden)]
    pub host_state: Option<crate::runtime::mem::LAny>,
}

impl Coro {
    /// PUC `traversethread`: the stack up to its top, what is above it
    /// cleared (a suspended thread's stack changes only once it runs again).
    pub(crate) fn trace(&mut self, m: &mut Marker) {
        m.value(self.body);
        let top = (self.gc_top as usize).min(self.stack.len());
        for &v in self.stack[..top].iter() {
            m.value(v);
        }
        self.stack[top..].fill(Value::Nil);
        for cf in self.frames.iter() {
            match cf {
                CallFrame::Lua(f) => {
                    m.mark(f.closure);
                }
                CallFrame::Cont(nc) => {
                    if let ContKind::Xpcall { handler, .. } = nc.kind {
                        m.value(handler);
                    }
                }
            }
        }
        if !matches!(m.dialect, Dialect::L51 | Dialect::L52) {
            for &(_, uv) in self.open_upvals.iter() {
                m.mark(uv);
            }
        }
        if let Some(r) = self.resumer {
            m.mark(r);
        }
        if let Some(e) = self.error_value {
            m.value(e);
        }
        if let Some(d) = &self.dead {
            d.trace(m);
        }
        if let Some(h) = self.hook.func {
            m.value(h);
        }
        m.value(Value::Table(self.globals));
        for &v in self.host_stack.iter() {
            m.value(v);
        }
    }
}
