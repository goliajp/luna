//! The call stack as PUC's debug interface walks it.
//!
//! PUC keeps one `CallInfo` per running function, Lua or C, and phrases
//! every debug query in those levels: `lua_getstack`, `getinfo`,
//! `getlocal`/`setlocal`, `luaL_where`, `luaL_traceback`. luna keeps Lua
//! activations and the continuations of the yieldable natives (pcall,
//! xpcall, pairs) on `frames`, and every other running native on
//! `running_natives`, each tagged with the frame depth it was entered at.
//! [`ThreadStack`] interleaves the two into PUC's order: a native entered at
//! depth `d` sits above `frames[d - 1]` and below `frames[d]`. A metamethod
//! or `__close` handler run by an instruction gets no C level between it
//! and that instruction's function, as in PUC. Nor does a host's protected
//! call (`Vm::call_value_with_handler`), which PUC's `lua_pcall` makes
//! without a `CallInfo` of its own.

use crate::runtime::function::{CallFrame, ContKind, Frame};
use crate::runtime::{Gc, NativeClosure, Value};

mod chunk_id;
mod levels;
mod raise;
mod traceback;

pub(crate) use chunk_id::{chunk_id, syntax_chunk_id};
pub(crate) use levels::tail_ar;
pub(crate) use raise::ErroredNative;
pub(crate) use traceback::traceback_from_lines;

/// Where a running native sits: its value-stack window and the number of
/// frames below it when it was entered.
#[derive(Clone, Copy)]
pub(crate) struct NativeAct {
    pub(crate) nc: Gc<NativeClosure>,
    pub(crate) func_slot: u32,
    pub(crate) nargs: u32,
    pub(crate) depth: u32,
    /// `__call` metamethods resolved to reach it (PUC 5.5 `CIST_CCMT`)
    pub(crate) ccmt: u8,
}

/// One stack level (one PUC `CallInfo`).
#[derive(Clone, Copy)]
pub(crate) enum DbgKind {
    /// a Lua activation, by index into the thread's `frames`
    Lua(usize),
    /// a 5.1 lost tail call: `lua_getstack` reports one level per tail call
    /// collapsed into the Lua activation above it
    Tail,
    /// a C function
    C(CLevel),
}

#[derive(Clone, Copy)]
pub(crate) enum CLevel {
    /// the thread's `i`-th running native
    Native(usize),
    /// the pcall / xpcall / pairs whose continuation is `frames[i]`
    Cont(usize),
    /// the `coroutine.yield` a suspended coroutine is parked in, by the
    /// stack slot of the call
    Yield(u32),
}

/// A local variable's storage, as `lua_getlocal` resolves it.
#[derive(Clone, Copy)]
pub(crate) enum LocalSlot {
    /// an actual stack slot of the thread
    Stack(usize),
    /// a value PUC's C library function keeps in a stack slot luna's native
    /// does not have (pcall's status boolean, xpcall's function copies)
    Held(Value),
}

/// PUC `lua_Debug`, filled for one level or one function value.
pub(crate) struct Ar {
    pub(crate) what: &'static str,
    pub(crate) source: Vec<u8>,
    pub(crate) short_src: Vec<u8>,
    pub(crate) linedefined: i64,
    pub(crate) lastlinedefined: i64,
    pub(crate) currentline: i64,
    /// `(namewhat, name)`; `None` is PUC's `namewhat = ""`, `name = NULL`
    pub(crate) name: Option<(&'static str, String)>,
    pub(crate) istailcall: bool,
    pub(crate) extraargs: i64,
    pub(crate) ftransfer: i64,
    pub(crate) ntransfer: i64,
    pub(crate) nups: i64,
    pub(crate) nparams: i64,
    pub(crate) isvararg: bool,
    /// the running function; `Nil` for a 5.1 lost tail call
    pub(crate) func: Value,
}

/// A read-only view of one thread's call stack.
pub(crate) struct ThreadStack<'a> {
    pub(crate) frames: &'a [CallFrame],
    pub(crate) stack: &'a [Value],
    top: u32,
    acts: &'a [NativeAct],
    /// level 0 first
    pub(crate) levels: Vec<DbgKind>,
}

impl<'a> ThreadStack<'a> {
    pub(crate) fn new(
        v51: bool,
        frames: &'a [CallFrame],
        stack: &'a [Value],
        top: u32,
        acts: &'a [NativeAct],
        yield_slot: Option<u32>,
    ) -> Self {
        let mut levels = Vec::with_capacity(frames.len() + acts.len() + 1);
        if let Some(fs) = yield_slot {
            levels.push(DbgKind::C(CLevel::Yield(fs)));
        }
        let mut k = acts.len();
        for p in (0..=frames.len()).rev() {
            while k > 0 && acts[k - 1].depth as usize == p {
                k -= 1;
                if !is_host_call(Value::Native(acts[k].nc)) {
                    levels.push(DbgKind::C(CLevel::Native(k)));
                }
            }
            if p == 0 {
                break;
            }
            match &frames[p - 1] {
                CallFrame::Lua(f) => {
                    levels.push(DbgKind::Lua(p - 1));
                    if v51 {
                        for _ in 0..f.tailcalls {
                            levels.push(DbgKind::Tail);
                        }
                    }
                }
                CallFrame::Cont(nc) => {
                    // a C function waiting on its continuation is a level
                    // of its own once a yield has taken it off the running
                    // natives; before that, the native is the level
                    let level = match nc.kind {
                        // a host's `lua_pcall` is no level: its callee sits
                        // on the slot (`callee_shift`)
                        ContKind::Pcall { level, .. } | ContKind::Xpcall { level, .. } => {
                            level && !is_host_call(stack[nc.func_slot as usize])
                        }
                        ContKind::Pairs => !is_host_call(stack[nc.func_slot as usize]),
                        ContKind::Host(_) => !acts.iter().any(|a| a.func_slot == nc.func_slot),
                        _ => false,
                    };
                    if level {
                        levels.push(DbgKind::C(CLevel::Cont(p - 1)));
                    }
                }
            }
        }
        ThreadStack {
            frames,
            stack,
            top,
            acts,
            levels,
        }
    }

    pub(crate) fn lua(&self, fi: usize) -> &'a Frame {
        self.frames[fi].lua().expect("Lua level")
    }

    fn cont_slot(&self, fi: usize) -> u32 {
        match &self.frames[fi] {
            CallFrame::Cont(nc) => nc.func_slot,
            CallFrame::Lua(_) => unreachable!("continuation level"),
        }
    }

    /// The function running at level `i` (PUC `ar.func`).
    pub(crate) fn func(&self, i: usize) -> Value {
        match self.levels[i] {
            DbgKind::Lua(fi) => Value::Closure(self.lua(fi).closure),
            DbgKind::Tail => Value::Nil,
            DbgKind::C(CLevel::Native(k)) => Value::Native(self.acts[k].nc),
            DbgKind::C(CLevel::Cont(fi)) => self.stack[self.cont_slot(fi) as usize],
            DbgKind::C(CLevel::Yield(fs)) => self.stack[fs as usize],
        }
    }

    /// The stack slot of the function at level `i`, which bounds the
    /// temporaries of the level below it (PUC `ci->next->func`).
    pub(crate) fn func_slot(&self, i: usize) -> Option<u32> {
        Some(match self.levels[i] {
            DbgKind::Lua(fi) => self.lua(fi).func_slot,
            DbgKind::Tail => return None,
            DbgKind::C(CLevel::Native(k)) => self.acts[k].func_slot,
            DbgKind::C(CLevel::Cont(fi)) => self.cont_slot(fi),
            DbgKind::C(CLevel::Yield(fs)) => fs,
        })
    }

    /// PUC `currentline`: -1 for a C function or a Lua function without
    /// line information.
    pub(crate) fn currentline(&self, i: usize) -> i64 {
        match self.levels[i] {
            DbgKind::Lua(fi) => frame_line(self.lua(fi)),
            _ => -1,
        }
    }

    /// PUC `CIST_TAIL` (5.2+): the activation was reused by a tail call.
    fn is_tail(&self, i: usize) -> bool {
        match self.levels[i] {
            DbgKind::Lua(fi) => self.lua(fi).tailcalls > 0,
            _ => false,
        }
    }

    fn is_hook(&self, i: usize) -> bool {
        matches!(self.levels[i], DbgKind::Lua(fi) if self.lua(fi).is_hook)
    }

    fn is_finalizer(&self, i: usize) -> bool {
        matches!(self.levels[i], DbgKind::Lua(fi) if self.lua(fi).tm == Some(crate::runtime::function::FrameTm::Gc))
    }

    /// The instruction a Lua level is executing (PUC `currentpc`), with its
    /// index. A function whose call hook is running has not executed
    /// anything yet; PUC's `callhook` bumps the saved pc for the hook, so it
    /// reads as the first instruction.
    fn current_instr(&self, fi: usize) -> Option<(usize, crate::vm::isa::Inst)> {
        let f = self.lua(fi);
        let pc = (f.pc as usize).max(1) - 1;
        Some((pc, *f.closure.proto.code.get(pc)?))
    }
}

/// Is `f` the protected call `Vm::call_value_with_handler` makes?
fn is_host_call(f: Value) -> bool {
    use crate::vm::exec::native_call::NativeKind;
    matches!(f, Value::Native(nc) if matches!(nc.kind, NativeKind::HostXpcall | NativeKind::HostPcall))
}

/// The line of the instruction `f` is executing; -1 without line info.
pub(crate) fn frame_line(f: &Frame) -> i64 {
    let proto = f.closure.proto;
    if proto.lines.is_empty() {
        return -1;
    }
    let pc = (f.pc as usize).saturating_sub(1).min(proto.lines.len() - 1);
    proto.lines[pc] as i64
}
