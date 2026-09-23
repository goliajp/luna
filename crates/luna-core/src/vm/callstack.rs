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
//! and that instruction's function, as in PUC.

use crate::runtime::function::{CallFrame, ContKind, Frame};
use crate::runtime::{Coro, CoroStatus, Gc, LuaClosure, NativeClosure, Table, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;
use crate::vm::isa::Op;

/// Where a running native sits: its value-stack window and the number of
/// frames below it when it was entered.
#[derive(Clone, Copy)]
pub(crate) struct NativeAct {
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
    natives: &'a [Gc<NativeClosure>],
    acts: &'a [NativeAct],
    /// level 0 first
    pub(crate) levels: Vec<DbgKind>,
}

impl<'a> ThreadStack<'a> {
    fn new(
        v51: bool,
        frames: &'a [CallFrame],
        stack: &'a [Value],
        top: u32,
        natives: &'a [Gc<NativeClosure>],
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
                levels.push(DbgKind::C(CLevel::Native(k)));
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
                    if matches!(
                        nc.kind,
                        ContKind::Pcall | ContKind::Xpcall { .. } | ContKind::Pairs
                    ) {
                        levels.push(DbgKind::C(CLevel::Cont(p - 1)));
                    }
                }
            }
        }
        ThreadStack {
            frames,
            stack,
            top,
            natives,
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
            DbgKind::C(CLevel::Native(k)) => Value::Native(self.natives[k]),
            DbgKind::C(CLevel::Cont(fi)) => self.stack[self.cont_slot(fi) as usize],
            DbgKind::C(CLevel::Yield(fs)) => self.stack[fs as usize],
        }
    }

    /// The stack slot of the function at level `i`, which bounds the
    /// temporaries of the level below it (PUC `ci->next->func`).
    fn func_slot(&self, i: usize) -> Option<u32> {
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
        matches!(self.levels[i], DbgKind::Lua(fi) if self.lua(fi).tm == Some("gc"))
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

/// The line of the instruction `f` is executing; -1 without line info.
pub(crate) fn frame_line(f: &Frame) -> i64 {
    let proto = f.closure.proto;
    if proto.lines.is_empty() {
        return -1;
    }
    let pc = (f.pc as usize).saturating_sub(1).min(proto.lines.len() - 1);
    proto.lines[pc] as i64
}

/// PUC `luaO_chunkid`: render a chunk's `source` into its `short_src` form,
/// truncated to `LUA_IDSIZE`. `=name` keeps the literal (head-truncated), `@file`
/// keeps the path (tail-truncated behind `...`), anything else is treated as a
/// string and wrapped as `[string "first line..."]`.
pub(crate) fn chunk_id(source: &[u8]) -> Vec<u8> {
    const IDSIZE: usize = 60;
    const RETS: &[u8] = b"...";
    const PRE: &[u8] = b"[string \"";
    const POS: &[u8] = b"\"]";
    let mut out = Vec::new();
    match source.first() {
        Some(b'=') => {
            // `srclen` counts the sigil, so a 60-byte `=NAME` still fits.
            let s = &source[1..];
            if source.len() <= IDSIZE {
                out.extend_from_slice(s);
            } else {
                out.extend_from_slice(&s[..IDSIZE - 1]);
            }
        }
        Some(b'@') => {
            let s = &source[1..];
            if source.len() <= IDSIZE {
                out.extend_from_slice(s);
            } else {
                out.extend_from_slice(RETS);
                let bufflen = IDSIZE - RETS.len() - 1;
                out.extend_from_slice(&s[s.len() - bufflen..]);
            }
        }
        _ => {
            let nl = source.iter().position(|&c| c == b'\n');
            out.extend_from_slice(PRE);
            let bufflen = IDSIZE - PRE.len() - RETS.len() - POS.len() - 1;
            let mut srclen = source.len();
            if srclen < bufflen && nl.is_none() {
                out.extend_from_slice(source);
            } else {
                if let Some(n) = nl {
                    srclen = n;
                }
                srclen = srclen.min(bufflen);
                out.extend_from_slice(&source[..srclen]);
                out.extend_from_slice(RETS);
            }
            out.extend_from_slice(POS);
        }
    }
    out
}

/// PUC's `tmname` for a debug name: 5.2/5.3 keep the `__`, 5.4+ drop it.
fn tm_name(v: LuaVersion, event: &str) -> String {
    if v <= LuaVersion::Lua53 {
        format!("__{event}")
    } else {
        event.to_string()
    }
}

/// The metamethod event an instruction can call, per PUC
/// `funcnamefromcode` of each version (5.2's `getfuncname`). 5.1 names no
/// metamethod.
fn instr_event(v: LuaVersion, op: Op) -> Option<&'static str> {
    Some(match op {
        Op::SelfOp | Op::GetTabUp | Op::GetTable | Op::GetI | Op::GetField => "index",
        Op::SetTabUp | Op::SetTable | Op::SetI | Op::SetField => "newindex",
        Op::Eq => "eq",
        Op::Add => "add",
        Op::Sub => "sub",
        Op::Mul => "mul",
        Op::Div => "div",
        Op::Mod => "mod",
        Op::Pow => "pow",
        Op::Unm => "unm",
        Op::Len => "len",
        Op::Lt => "lt",
        Op::Le => "le",
        Op::Concat => "concat",
        Op::IDiv if v >= LuaVersion::Lua53 => "idiv",
        Op::BAnd if v >= LuaVersion::Lua53 => "band",
        Op::BOr if v >= LuaVersion::Lua53 => "bor",
        Op::BXor if v >= LuaVersion::Lua53 => "bxor",
        Op::Shl if v >= LuaVersion::Lua53 => "shl",
        Op::Shr if v >= LuaVersion::Lua53 => "shr",
        Op::BNot if v >= LuaVersion::Lua53 => "bnot",
        Op::Close | Op::Return if v >= LuaVersion::Lua54 => "close",
        _ => return None,
    })
}

impl Vm {
    /// The call stack of `co`, or of the running thread for `None` (or when
    /// `co` is the running coroutine).
    pub(crate) fn thread_stack(&self, co: Option<Gc<Coro>>) -> ThreadStack<'_> {
        let v51 = self.version() <= LuaVersion::Lua51;
        match co {
            Some(co) if !self.is_current_thread(Some(co)) => {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is
                // single-threaded and `co` is reachable (the caller holds it
                // as a native argument) for as long as the view lives, and
                // nothing mutates a non-running coroutine meanwhile.
                let c: &Coro = unsafe { &*co.as_ptr() };
                let yield_slot = match c.status {
                    CoroStatus::Suspended => c.resume_at.map(|(fs, _)| fs),
                    _ => None,
                };
                // a normal coroutine is inside the natives that resumed
                // another (at least `coroutine.resume` itself)
                let natives = match c.status {
                    CoroStatus::Normal => c.natives.clone(),
                    _ => 0..0,
                };
                ThreadStack::new(
                    v51,
                    &c.frames,
                    &c.stack,
                    c.top,
                    &self.running_natives[natives.clone()],
                    &self.running_native_acts[natives],
                    yield_slot,
                )
            }
            _ => ThreadStack::new(
                v51,
                &self.frames,
                &self.stack,
                self.top,
                &self.running_natives[self.natives_base..],
                &self.running_native_acts[self.natives_base..],
                None,
            ),
        }
    }

    /// The running thread's level `level` (PUC `lua_getstack`), level 0 being
    /// the running native.
    pub(crate) fn dbg_frame(&self, level: i64) -> Option<DbgKind> {
        let ts = self.thread_stack(None);
        usize::try_from(level)
            .ok()
            .and_then(|i| ts.levels.get(i).copied())
    }

    /// The Lua closure running at `level` on the current thread, or `None`
    /// for a C level. PUC 5.1 `getfenv`/`setfenv` need this to reach the
    /// function whose env they read or rewrite.
    pub(crate) fn lua_closure_at_level(&self, level: i64) -> Option<Gc<LuaClosure>> {
        let ts = self.thread_stack(None);
        match ts.levels.get(usize::try_from(level).ok()?)? {
            DbgKind::Lua(fi) => Some(ts.lua(*fi).closure),
            _ => None,
        }
    }

    /// PUC `getfuncname` of each version: how the level below `i` named the
    /// function running at `i`.
    pub(crate) fn level_name(&self, ts: &ThreadStack, i: usize) -> Option<(&'static str, String)> {
        let v = self.version();
        if matches!(ts.levels[i], DbgKind::Tail) {
            return None;
        }
        if v == LuaVersion::Lua53 && i > 0 && ts.is_finalizer(i - 1) {
            // 5.3 flags the level the collector interrupted (`CIST_FIN`)
            // and names *it* after the finalizer.
            return Some(("metamethod", "__gc".to_string()));
        }
        if ts.is_tail(i) {
            return None;
        }
        if v >= LuaVersion::Lua54 {
            if ts.is_hook(i) {
                return Some(("hook", "?".to_string()));
            }
            if ts.is_finalizer(i) {
                return Some(("metamethod", "__gc".to_string()));
            }
        }
        let DbgKind::Lua(caller) = *ts.levels.get(i + 1)? else {
            return None;
        };
        if v == LuaVersion::Lua53 && ts.is_hook(i) {
            return Some(("hook", "?".to_string()));
        }
        let (pc, instr) = ts.current_instr(caller)?;
        let p = &ts.lua(caller).closure.proto;
        match instr.op() {
            Op::Call | Op::TailCall => crate::vm::objname::getobjname(p, pc, instr.a()),
            Op::TForCall if v == LuaVersion::Lua51 => {
                crate::vm::objname::getobjname(p, pc, instr.a())
            }
            Op::TForCall => Some(("for iterator", "for iterator".to_string())),
            op if v >= LuaVersion::Lua52 => {
                instr_event(v, op).map(|e| ("metamethod", tm_name(v, e)))
            }
            _ => None,
        }
    }

    /// PUC `lua_getinfo` for level `i` of `ts`, all options filled.
    pub(crate) fn level_ar(&self, ts: &ThreadStack, i: usize) -> Ar {
        let v = self.version();
        let func = ts.func(i);
        let mut ar = match ts.levels[i] {
            DbgKind::Tail => return tail_ar(),
            DbgKind::Lua(fi) => {
                let f = ts.lua(fi);
                let mut ar = self.closure_ar(f.closure);
                ar.currentline = ts.currentline(i);
                ar.istailcall = ts.is_tail(i);
                ar.extraargs = f.ccmt as i64;
                ar
            }
            DbgKind::C(c) => {
                let mut ar = self.function_ar(func);
                if let CLevel::Native(k) = c {
                    ar.extraargs = ts.acts[k].ccmt as i64;
                }
                ar
            }
        };
        ar.name = self.level_name(ts, i);
        // PUC fills 'r' only for the level a call/return hook interrupted
        // (5.4 `CIST_TRAN`, set when values are transferred; 5.5 for every
        // hook event, with the transfer the event carried).
        if i > 0 && ts.is_hook(i - 1) && (v >= LuaVersion::Lua55 || self.hook_ntransfer != 0) {
            ar.ftransfer = self.hook_ftransfer as i64;
            ar.ntransfer = self.hook_ntransfer as i64;
        }
        ar
    }

    /// PUC `lua_getinfo(">...")` on a function value.
    pub(crate) fn function_ar(&self, f: Value) -> Ar {
        match f {
            Value::Closure(cl) => self.closure_ar(cl),
            Value::Native(nc) => Ar {
                what: "C",
                source: b"=[C]".to_vec(),
                short_src: b"[C]".to_vec(),
                linedefined: -1,
                lastlinedefined: -1,
                currentline: -1,
                name: None,
                istailcall: false,
                extraargs: 0,
                ftransfer: 0,
                ntransfer: 0,
                nups: nc.upvals.len() as i64,
                nparams: 0,
                isvararg: true,
                func: f,
            },
            _ => unreachable!("a function value"),
        }
    }

    pub(crate) fn closure_ar(&self, cl: Gc<LuaClosure>) -> Ar {
        let proto = cl.proto;
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let raw = unsafe { crate::runtime::string::bytes_of(proto.source.as_ptr()) };
        // PUC `funcinfo` substitutes "=?" for a Proto without a source (a
        // stripped binary chunk); luna marks that as no source and no line
        // table, so a text chunk named "" still reads `[string ""]`.
        let source: Vec<u8> = if raw.is_empty() && proto.lines.is_empty() {
            b"=?".to_vec()
        } else {
            raw.to_vec()
        };
        // 5.1 functions keep their environment outside the upvalues, so
        // `_ENV` (which luna keeps in a cell) is not counted.
        let nups = if self.version() <= LuaVersion::Lua51 {
            proto.upvals.iter().filter(|u| &*u.name != "_ENV").count() as i64
        } else {
            cl.upvals().len() as i64
        };
        Ar {
            what: if proto.line_defined == 0 {
                "main"
            } else {
                "Lua"
            },
            short_src: chunk_id(&source),
            source,
            linedefined: proto.line_defined as i64,
            lastlinedefined: proto.last_line_defined as i64,
            currentline: -1,
            name: None,
            istailcall: false,
            extraargs: 0,
            ftransfer: 0,
            ntransfer: 0,
            nups,
            nparams: proto.num_params as i64,
            isvararg: proto.is_vararg,
            func: Value::Closure(cl),
        }
    }

    /// PUC `luaG_findlocal` / 5.1 `findlocal`: the `n`-th local of level `i`
    /// (negative `n`: the varargs, 5.2+) and where it lives.
    pub(crate) fn find_local(
        &self,
        ts: &ThreadStack,
        i: usize,
        n: i64,
    ) -> Option<(String, LocalSlot)> {
        let (base, reg) = match ts.levels[i] {
            DbgKind::Tail => return None,
            DbgKind::Lua(fi) => {
                let f = ts.lua(fi);
                if n < 0 {
                    if self.version() == LuaVersion::Lua51 {
                        return None;
                    }
                    let k = n.unsigned_abs();
                    if k > f.n_varargs as u64 {
                        return None;
                    }
                    let slot = (f.func_slot as u64 + k) as usize;
                    let name = self.vararg_locvar_name().to_string();
                    return Some((name, LocalSlot::Stack(slot)));
                }
                if let Some((name, reg)) = self.named_local(f, n) {
                    let at = reg.map_or(LocalSlot::Held(Value::Nil), |r| {
                        LocalSlot::Stack((f.base + r) as usize)
                    });
                    return Some((name, at));
                }
                // 5.5's `(vararg table)` has a register in PUC, not in luna
                let pseudo = f.closure.proto.has_vararg_table_pseudo
                    && n > f.closure.proto.num_params as i64 + 1;
                (f.base, n - 1 - i64::from(pseudo))
            }
            DbgKind::C(CLevel::Cont(fi)) => {
                let held = self.cont_temporaries(ts, fi);
                let k = usize::try_from(n).ok()?.checked_sub(1)?;
                let val = *held.get(k)?;
                let name = self.temporary_locvar_name().to_string();
                return Some((name, LocalSlot::Held(val)));
            }
            DbgKind::C(CLevel::Native(k)) => (ts.acts[k].func_slot + 1, n - 1),
            DbgKind::C(CLevel::Yield(fs)) => (fs + 1, n - 1),
        };
        // Past the named locals, anything up to the next level's function
        // (or the top, for the running level) is a temporary. A native's
        // window is the arguments it was given.
        let limit = match ts.levels[i] {
            DbgKind::C(CLevel::Native(k)) => ts.acts[k].func_slot + 1 + ts.acts[k].nargs,
            _ if i == 0 => ts.top.max(base),
            _ => ts.func_slot(i - 1)?,
        };
        if n < 1 || reg < 0 || base as i64 + reg >= limit as i64 {
            return None;
        }
        let name = match ts.levels[i] {
            DbgKind::Lua(_) => self.lua_temporary_locvar_name(),
            _ => self.temporary_locvar_name(),
        };
        Some((
            name.to_string(),
            LocalSlot::Stack((base as i64 + reg) as usize),
        ))
    }

    /// Named locals of a Lua frame (PUC `luaF_getlocalname` at the current
    /// pc), with 5.5's hidden `(vararg table)` slot: `(name, register)`,
    /// the register being `None` for that storage-less slot.
    fn named_local(&self, f: &Frame, n: i64) -> Option<(String, Option<u32>)> {
        let proto = f.closure.proto;
        let vararg_slot = proto
            .has_vararg_table_pseudo
            .then_some(proto.num_params as i64 + 1);
        if vararg_slot == Some(n) {
            return Some(("(vararg table)".to_string(), None));
        }
        let pc = (f.pc as usize).saturating_sub(1);
        let mut active: Vec<&crate::runtime::LocVar> = proto
            .locvars
            .iter()
            .filter(|lv| (lv.start_pc as usize) <= pc && pc < lv.end_pc as usize)
            .collect();
        active.sort_by_key(|lv| (lv.start_pc, lv.reg));
        let mut idx = n.checked_sub(1)?;
        if vararg_slot.is_some_and(|vs| n > vs) {
            idx -= 1;
        }
        let lv = active.get(usize::try_from(idx).ok()?)?;
        Some((lv.name.to_string(), Some(lv.reg)))
    }

    /// The values PUC's pcall / xpcall / pairs keep below the function they
    /// call — their C temporaries (`luaB_pcall` inserts its status boolean,
    /// `luaB_xpcall` 5.3+ also its function and handler, 5.1/5.2 the handler).
    fn cont_temporaries(&self, ts: &ThreadStack, fi: usize) -> Vec<Value> {
        let v = self.version();
        let CallFrame::Cont(nc) = &ts.frames[fi] else {
            unreachable!("continuation level")
        };
        match nc.kind {
            ContKind::Pcall if v == LuaVersion::Lua51 => vec![],
            ContKind::Pcall if v == LuaVersion::Lua52 => vec![Value::Nil],
            ContKind::Pcall => vec![Value::Bool(true)],
            ContKind::Xpcall { handler } if v <= LuaVersion::Lua52 => vec![handler],
            ContKind::Xpcall { handler } => {
                vec![
                    ts.stack[(nc.func_slot + 1) as usize],
                    handler,
                    Value::Bool(true),
                ]
            }
            _ => vec![],
        }
    }

    /// Write stack slot `slot` of thread `co` (`None`: the running one), as
    /// `lua_setlocal` does.
    pub(crate) fn write_thread_slot(&mut self, co: Option<Gc<Coro>>, slot: usize, v: Value) {
        let stack = match co {
            Some(co) if !self.is_current_thread(Some(co)) => {
                // the coroutine's saved stack is traced through `co`; re-gray it
                self.heap
                    .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { &mut co.as_mut().stack }
            }
            _ => &mut self.stack,
        };
        if stack.len() <= slot {
            stack.resize(slot + 1, Value::Nil);
        }
        stack[slot] = v;
    }

    /// PUC `pushglobalfuncname`: the name `f` is reachable by, two tables
    /// deep, from the loaded modules (5.3+, dropping a `_G.` prefix) or the
    /// global table (5.2, where PUC's `_G.` spelling depends on hash order
    /// and luna keeps the short form).
    pub(crate) fn global_func_name(&mut self, f: Value) -> Option<String> {
        let root = if self.version() == LuaVersion::Lua52 {
            self.globals()
        } else {
            let pkg_k = Value::Str(self.heap.intern(b"package"));
            let Value::Table(pkg) = self.globals().get(pkg_k) else {
                return None;
            };
            let loaded_k = Value::Str(self.heap.intern(b"loaded"));
            let Value::Table(loaded) = pkg.get(loaded_k) else {
                return None;
            };
            loaded
        };
        let name = find_field(root, f, 2)?;
        Some(match name.strip_prefix("_G.") {
            Some(rest) => rest.to_string(),
            None => name,
        })
    }
}

/// PUC `findfield`: a string-keyed path to `f` at most `level` tables deep,
/// in the table's traversal order.
fn find_field(t: Gc<Table>, f: Value, level: u32) -> Option<String> {
    if level == 0 {
        return None;
    }
    let mut k = Value::Nil;
    while let Ok(Some((nk, nv))) = t.next(k) {
        k = nk;
        let Value::Str(key) = nk else { continue };
        let key = String::from_utf8_lossy(key.as_bytes());
        if nv.raw_eq(f) {
            return Some(key.into_owned());
        }
        if let Value::Table(inner) = nv
            && let Some(rest) = find_field(inner, f, level - 1)
        {
            return Some(format!("{key}.{rest}"));
        }
    }
    None
}

/// PUC 5.1 `info_tailcall`: the placeholder a lost tail call reports.
pub(crate) fn tail_ar() -> Ar {
    Ar {
        what: "tail",
        source: b"=(tail call)".to_vec(),
        short_src: b"(tail call)".to_vec(),
        linedefined: -1,
        lastlinedefined: -1,
        currentline: -1,
        name: Some(("", String::new())),
        istailcall: false,
        extraargs: 0,
        ftransfer: 0,
        ntransfer: 0,
        nups: 0,
        nparams: 0,
        isvararg: false,
        func: Value::Nil,
    }
}

/// One line of a traceback: a level, or where levels were left out.
enum TbLine {
    Level(i64),
    /// `\n\t...` (5.1-5.3)
    Dots,
    /// `\n\t...\t(skipping N levels)` (5.4+)
    Skip(i64),
}

/// The lines `luaL_traceback` (5.1: `db_errorfb`) prints for a stack of `n`
/// levels starting at `level`, replaying each version's elision loop.
fn traceback_plan(v: LuaVersion, n: i64, mut level: i64) -> Vec<TbLine> {
    // 5.1's `lua_getstack` answers a negative level with a lost tail call
    let valid = |l: i64| l < n && (l >= 0 || v == LuaVersion::Lua51);
    // PUC `lastlevel` / `countlevels`: the deepest valid level, at least 0
    let last = (n - 1).max(0);
    let mut out = Vec::new();
    match v {
        LuaVersion::Lua51 => {
            const LEVELS1: i64 = 12;
            const LEVELS2: i64 = 10;
            let mut firstpart = true;
            loop {
                let l = level;
                level += 1;
                if !valid(l) {
                    break;
                }
                if level > LEVELS1 && firstpart {
                    if !valid(level + LEVELS2) {
                        level -= 1;
                    } else {
                        out.push(TbLine::Dots);
                        while valid(level + LEVELS2) {
                            level += 1;
                        }
                    }
                    firstpart = false;
                    continue;
                }
                out.push(TbLine::Level(l));
            }
        }
        LuaVersion::Lua52 => {
            const LEVELS1: i64 = 12;
            const LEVELS2: i64 = 10;
            let mark = if last > LEVELS1 + LEVELS2 { LEVELS1 } else { 0 };
            loop {
                let l = level;
                level += 1;
                if !valid(l) {
                    break;
                }
                if level == mark {
                    out.push(TbLine::Dots);
                    level = last - LEVELS2;
                } else {
                    out.push(TbLine::Level(l));
                }
            }
        }
        _ => {
            const LEVELS1: i64 = 10;
            const LEVELS2: i64 = 11;
            let mut limit = if last - level > LEVELS1 + LEVELS2 {
                LEVELS1
            } else {
                -1
            };
            loop {
                let l = level;
                level += 1;
                if !valid(l) {
                    break;
                }
                let elide = limit == 0;
                limit -= 1;
                if !elide {
                    out.push(TbLine::Level(l));
                } else if v == LuaVersion::Lua53 {
                    out.push(TbLine::Dots);
                    level = last - LEVELS2 + 1;
                } else {
                    let skip = last - level - LEVELS2 + 1;
                    out.push(TbLine::Skip(skip));
                    level += skip;
                }
            }
        }
    }
    out
}

impl Vm {
    /// The level lines of `luaL_traceback(L, L1, NULL, level)` (5.1
    /// `db_errorfb`) for thread `co` (`None`: the running one), each starting
    /// with `\n\t` — everything after the `stack traceback:` header.
    pub(crate) fn traceback_lines(&mut self, co: Option<Gc<Coro>>, level: i64) -> Vec<u8> {
        let v = self.version();
        let (plan, ars) = {
            let ts = self.thread_stack(co);
            let plan = traceback_plan(v, ts.levels.len() as i64, level);
            let ars: Vec<Ar> = plan
                .iter()
                .filter_map(|line| match *line {
                    TbLine::Level(l) if l < 0 => Some(tail_ar()),
                    TbLine::Level(l) => Some(self.level_ar(&ts, l as usize)),
                    _ => None,
                })
                .collect();
            (plan, ars)
        };
        let mut ars = ars.into_iter();
        let mut names = GlobalNames::default();
        let mut out = Vec::new();
        for line in plan {
            match line {
                TbLine::Dots => out.extend_from_slice(b"\n\t..."),
                TbLine::Skip(n) => {
                    out.extend_from_slice(format!("\n\t...\t(skipping {n} levels)").as_bytes())
                }
                TbLine::Level(_) => {
                    let ar = ars.next().expect("one Ar per level line");
                    self.traceback_line(&ar, &mut out, &mut names);
                }
            }
        }
        out
    }

    /// The traceback line of every level of the running thread, level 0
    /// first — a snapshot `traceback_from_lines` can elide from any level.
    pub(crate) fn level_lines(&mut self) -> Vec<Vec<u8>> {
        let ars: Vec<Ar> = {
            let ts = self.thread_stack(None);
            (0..ts.levels.len())
                .map(|i| self.level_ar(&ts, i))
                .collect()
        };
        let mut names = GlobalNames::default();
        ars.iter()
            .map(|ar| {
                let mut line = Vec::new();
                self.traceback_line(ar, &mut line, &mut names);
                line
            })
            .collect()
    }

    fn traceback_line(&mut self, ar: &Ar, out: &mut Vec<u8>, names: &mut GlobalNames) {
        let v = self.version();
        out.extend_from_slice(b"\n\t");
        out.extend_from_slice(&ar.short_src);
        out.push(b':');
        if ar.currentline > 0 {
            out.extend_from_slice(format!("{}:", ar.currentline).as_bytes());
        }
        let name = ar.name.as_ref().filter(|(what, _)| !what.is_empty());
        let where_: String = if v == LuaVersion::Lua51 {
            match name {
                Some((_, n)) => format!(" in function '{n}'"),
                None if ar.what == "main" => " in main chunk".to_string(),
                None if matches!(ar.what, "C" | "tail") => " ?".to_string(),
                None => format!(" in function <{}:{}>", lossy(&ar.short_src), ar.linedefined),
            }
        } else {
            format!(" in {}", self.traceback_funcname(ar, names))
        };
        out.extend_from_slice(where_.as_bytes());
        if ar.istailcall && v >= LuaVersion::Lua52 {
            out.extend_from_slice(b"\n\t(...tail calls...)");
        }
    }

    /// PUC `pushfuncname` of 5.2 to 5.5.
    fn traceback_funcname(&mut self, ar: &Ar, names: &mut GlobalNames) -> String {
        let v = self.version();
        let name = ar.name.as_ref().filter(|(what, _)| !what.is_empty());
        let lua_fallback = || format!("function <{}:{}>", lossy(&ar.short_src), ar.linedefined);
        match v {
            LuaVersion::Lua52 => match name {
                Some((_, n)) => format!("function '{n}'"),
                None if ar.what == "main" => "main chunk".to_string(),
                None if ar.what == "C" => match names.lookup(self, ar.func) {
                    Some(g) => format!("function '{g}'"),
                    None => "?".to_string(),
                },
                None => lua_fallback(),
            },
            LuaVersion::Lua53 | LuaVersion::Lua54 => {
                if let Some(g) = names.lookup(self, ar.func) {
                    return format!("function '{g}'");
                }
                match name {
                    Some((what, n)) => format!("{what} '{n}'"),
                    None if ar.what == "main" => "main chunk".to_string(),
                    None if ar.what != "C" => lua_fallback(),
                    None => "?".to_string(),
                }
            }
            _ => match name {
                Some((what, n)) => format!("{what} '{n}'"),
                None if ar.what == "main" => "main chunk".to_string(),
                None => match names.lookup(self, ar.func) {
                    Some(g) => format!("function '{g}'"),
                    None if ar.what != "C" => lua_fallback(),
                    None => "?".to_string(),
                },
            },
        }
    }
}

/// `luaL_traceback`'s level lines, from `level`, over a snapshot taken by
/// `level_lines`.
pub(crate) fn traceback_from_lines(v: LuaVersion, lines: &[Vec<u8>], level: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for line in traceback_plan(v, lines.len() as i64, level) {
        match line {
            TbLine::Dots => out.extend_from_slice(b"\n\t..."),
            TbLine::Skip(n) => {
                out.extend_from_slice(format!("\n\t...\t(skipping {n} levels)").as_bytes())
            }
            // 5.1's lost tail call at a negative level
            TbLine::Level(l) if l < 0 => out.extend_from_slice(b"\n\t(tail call): ?"),
            TbLine::Level(l) => out.extend_from_slice(&lines[l as usize]),
        }
    }
    out
}

/// `pushglobalfuncname` results for one traceback, by function identity:
/// a deep stack repeats few functions, and each lookup walks the loaded
/// modules.
#[derive(Default)]
struct GlobalNames(std::collections::HashMap<usize, Option<String>>);

impl GlobalNames {
    fn lookup(&mut self, vm: &mut Vm, f: Value) -> Option<String> {
        let key = match f {
            Value::Closure(c) => c.as_ptr() as usize,
            Value::Native(n) => n.as_ptr() as usize,
            _ => return None,
        };
        if let Some(name) = self.0.get(&key) {
            return name.clone();
        }
        let name = vm.global_func_name(f);
        self.0.insert(key, name.clone());
        name
    }
}

fn lossy(b: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(b)
}

/// A native that raised the error in flight, and where it ran.
#[derive(Clone, Copy)]
pub(crate) struct ErroredNative {
    nc: Gc<NativeClosure>,
    act: NativeAct,
    err: Value,
}

impl Vm {
    /// PUC `luaG_runerror`: `msg` with the position of the running function
    /// when that is a Lua function; a running native adds none.
    pub(crate) fn runerror(&mut self, msg: &str) -> crate::vm::error::LuaError {
        let native_on_top = self.running_native_acts.len() > self.natives_base
            && self
                .running_native_acts
                .last()
                .is_some_and(|a| a.depth as usize == self.frames.len());
        if native_on_top {
            self.plain_err(msg)
        } else {
            self.rt_err(msg)
        }
    }

    /// Remember that `nc` raised `err`. A native only leaves the stack by
    /// the time the error reaches `unwind`, where PUC still has it; natives
    /// an error passes through on its way out collect innermost first, and
    /// a different error starts the list over.
    pub(crate) fn note_errored_native(
        &mut self,
        nc: Gc<NativeClosure>,
        act: NativeAct,
        err: Value,
    ) {
        let continues = self
            .errored_natives
            .last()
            .is_some_and(|inner| inner.err.raw_eq(err) && inner.act.depth >= act.depth);
        if !continues {
            self.errored_natives.clear();
        }
        self.errored_natives.push(ErroredNative { nc, act, err });
    }

    /// The natives recorded for `err` that were running at the top of the
    /// stack, innermost first.
    fn take_errored_natives(&mut self, err: Value) -> Vec<ErroredNative> {
        let mut list = std::mem::take(&mut self.errored_natives);
        let depth = self.frames.len() as u32;
        if !list
            .iter()
            .all(|e| e.err.raw_eq(err) && e.act.depth == depth)
        {
            list.clear();
        }
        list
    }

    /// The pcall (`Some(None)`) or xpcall (`Some(Some(handler))`) that will
    /// catch an error raised now, if any is in reach.
    fn nearest_catcher(&self) -> Option<Option<Value>> {
        let floor = self.msgh_floor.min(self.frames.len());
        self.frames[floor..].iter().rev().find_map(|cf| match cf {
            CallFrame::Cont(nc) => match nc.kind {
                ContKind::Pcall => Some(None),
                ContKind::Xpcall { handler } => Some(Some(handler)),
                _ => None,
            },
            CallFrame::Lua(_) => None,
        })
    }

    /// PUC `luaG_errormsg`, at the point the error reaches the unwinder:
    /// with the stack that raised it still in place (natives included), run
    /// the handler of the xpcall that will catch it and return the value
    /// that replaces the error. An error nothing in this thread will catch
    /// keeps its traceback for the host and for `debug.traceback` of a dead
    /// coroutine.
    pub(crate) fn raise_to_handler(&mut self, err: Value) -> Value {
        let raised_by = self.take_errored_natives(err);
        let base = self.running_natives.len();
        for e in raised_by.iter().rev() {
            self.running_natives.push(e.nc);
            self.running_native_acts.push(e.act);
        }
        let catcher = self.nearest_catcher();
        let to_host = catcher.is_none() && self.current.is_none() && self.keep_error_traceback;
        let out = match catcher {
            Some(Some(handler)) if !self.msgh_applied.is_some_and(|v| v.raw_eq(err)) => {
                let r = self.call_msgh(handler, err);
                self.msgh_applied = Some(r);
                r
            }
            None => {
                if self.keep_error_traceback && self.error_traceback.is_none() {
                    self.error_traceback = Some(self.level_lines());
                }
                err
            }
            Some(_) => err,
        };
        // 5.5 `luaG_errormsg` names a nil error object after any handler
        // ran; an error reaching the host keeps it for lua.c's handler
        let out = if out.is_nil() && self.version() >= LuaVersion::Lua55 && !to_host {
            Value::Str(self.heap.intern(b"<no error object>"))
        } else {
            out
        };
        self.running_natives.truncate(base);
        self.running_native_acts.truncate(base);
        out
    }

    /// Run an xpcall message handler on `err`. An error inside the handler
    /// calls it again with the new error, as PUC's `luaG_errormsg` re-enters
    /// the handler; after `MAX_C_DEPTH` reruns the error becomes "C stack
    /// overflow", and if the handler fails on that too, "error in error
    /// handling" (errors.lua :637).
    pub(crate) fn call_msgh(&mut self, handler: Value, err: Value) -> Value {
        let mut cur = err;
        let mut capped = false;
        for iter in 0.. {
            if iter >= crate::vm::exec::MAX_C_DEPTH && !capped {
                cur = Value::Str(self.heap.intern(b"C stack overflow"));
                capped = true;
            }
            self.msgh_depth += 1;
            let r = self.call_protected(handler, &[cur]);
            self.msgh_depth -= 1;
            match r {
                Ok(results) => return results.first().copied().unwrap_or(Value::Nil),
                Err(_) if capped => break,
                Err(e) => cur = e.0,
            }
        }
        Value::Str(self.heap.intern(b"error in error handling"))
    }

    /// `call_value` as a protected call made from Rust (PUC `lua_pcall`
    /// with no handler): errors inside it do not reach the handler of an
    /// enclosing xpcall, and its error bookkeeping does not outlive it.
    pub(crate) fn call_protected(
        &mut self,
        f: Value,
        args: &[Value],
    ) -> Result<Vec<Value>, crate::vm::error::LuaError> {
        let floor = std::mem::replace(&mut self.msgh_floor, self.frames.len());
        let applied = self.msgh_applied.take();
        let traceback = self.error_traceback.take();
        let natives = std::mem::take(&mut self.errored_natives);
        let keep = std::mem::replace(&mut self.keep_error_traceback, false);
        let r = self.call_value(f, args);
        self.keep_error_traceback = keep;
        self.msgh_floor = floor;
        self.msgh_applied = applied;
        self.error_traceback = traceback;
        self.errored_natives = natives;
        r
    }
}
