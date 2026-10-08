//! Per-level debug queries: names, `lua_Debug` records, locals.

use crate::runtime::function::{CallFrame, ContKind, Frame};
use crate::runtime::{Coro, CoroStatus, Gc, LuaClosure, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;
use crate::vm::isa::Op;
use crate::vm::objname::instr_event;

use super::chunk_id::chunk_id;
use super::{Ar, CLevel, DbgKind, LocalSlot, ThreadStack};

/// PUC's `tmname` for a debug name: 5.2/5.3 keep the `__`, 5.4+ drop it.
fn tm_name(v: LuaVersion, event: &str) -> String {
    if v <= LuaVersion::Lua53 {
        format!("__{event}")
    } else {
        event.to_string()
    }
}

impl Vm {
    /// The call stack of `co`, or of the running thread for `None` (or when
    /// `co` is the running coroutine).
    pub(crate) fn thread_stack(&self, co: Option<Gc<Coro>>) -> ThreadStack<'_> {
        let v51 = self.version() <= LuaVersion::Lua51;
        match co {
            Some(co) if !self.is_current_thread(Some(co)) => {
                // SAFETY: `co` stays allocated while the view lives (the
                // caller holds it as a native argument, and no collect runs
                // while the view borrows `self`); it is not the running
                // thread, and nothing writes a non-running coroutine while
                // `&self` is held, so the shared reference is not aliased
                let c: &Coro = unsafe { &*co.as_ptr() };
                // a C function that yielded with a continuation is the level
                // itself (its `ContKind::Host` frame), not a `coroutine.yield`
                let parked_in_c = matches!(
                    c.frames.last(),
                    Some(CallFrame::Cont(nc)) if matches!(nc.kind, ContKind::Host(_))
                );
                let yield_slot = match c.status {
                    CoroStatus::Suspended if !parked_in_c => c
                        .resume_at
                        .map(|(fs, _)| fs)
                        .filter(|&fs| fs != crate::vm::exec::HOOK_YIELD_SLOT),
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
                    &self.running_natives[natives],
                    yield_slot,
                )
            }
            _ => ThreadStack::new(
                v51,
                &self.frames,
                &self.stack,
                self.top,
                &self.running_natives[self.natives_base..],
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
            Op::Call | Op::TailCall => crate::vm::objname::getobjname_in(p, pc, instr.a(), v),
            op if op.is_tfor_call() && v == LuaVersion::Lua51 => {
                crate::vm::objname::getobjname_in(p, pc, instr.a(), v)
            }
            op if op.is_tfor_call() => Some(("for iterator", "for iterator".to_string())),
            _ if v >= LuaVersion::Lua52 => {
                instr_event(v, instr.source_op()).map(|e| ("metamethod", tm_name(v, e)))
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
                    ar.extraargs = ts.acts[k].ccmt() as i64;
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
        let raw = proto.source.as_bytes();
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
            (proto.upvals.len() - usize::from(proto.env_upval_idx != u8::MAX)) as i64
        } else {
            cl.upvals().len() as i64
        };
        Ar {
            what: if proto.line_defined == 0 {
                "main"
            } else {
                "Lua"
            },
            short_src: chunk_id(self.version(), &source),
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
                    let slot = (u64::from(f.base - f.n_varargs) + k - 1) as usize;
                    let name = self.vararg_locvar_name().to_string();
                    return Some((name, LocalSlot::Stack(slot)));
                }
                if let Some((name, r)) = self.named_local(f, n) {
                    let slot = f.base + r;
                    // a loaded chunk may name any register; as in PUC, a
                    // level's locals end below the function it is calling
                    if i > 0
                        && let Some(limit) = ts.func_slot(i - 1)
                        && slot >= limit
                    {
                        return None;
                    }
                    return Some((name, LocalSlot::Stack(slot as usize)));
                }
                (f.base, n - 1)
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
    /// pc): `(name, register)`.
    fn named_local(&self, f: &Frame, n: i64) -> Option<(String, u32)> {
        let proto = f.closure.proto;
        let pc = (f.pc as usize).saturating_sub(1);
        let mut active: Vec<&crate::runtime::LocVar> = proto
            .locvars
            .iter()
            .filter(|lv| (lv.start_pc as usize) <= pc && pc < lv.end_pc as usize)
            .collect();
        active.sort_by_key(|lv| (lv.start_pc, lv.reg));
        let idx = n.checked_sub(1)?;
        let lv = active.get(usize::try_from(idx).ok()?)?;
        Some((lv.name.to_string(), lv.reg))
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
            ContKind::Pcall { .. } if v == LuaVersion::Lua51 => vec![],
            ContKind::Pcall { .. } if v == LuaVersion::Lua52 => vec![Value::Nil],
            ContKind::Pcall { .. } => vec![Value::Bool(true)],
            ContKind::Xpcall { handler, .. } if v <= LuaVersion::Lua52 => vec![handler],
            ContKind::Xpcall { handler, .. } => {
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
                self.heap.barrier_back(co);
                // SAFETY: `co` is a suspended thread the caller holds (a native argument), not the running one, so the Vm holds no reference into its saved stack; the borrow lives until the slot is written, which does not collect
                unsafe { &mut co.as_mut().stack }
            }
            _ => &mut self.stack,
        };
        if stack.len() <= slot {
            stack.resize_or_abort(slot + 1, Value::Nil);
        }
        stack[slot] = v;
    }
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
