//! A traceback made as PUC makes it, string by string: 5.1's
//! `db_errorfb` and 5.2/5.3's `luaL_traceback` push the pieces of each
//! level and join everything so far (`lua_concat`); 5.4 and 5.5 add them
//! to a `luaL_Buffer`. Naming a function by where the loaded modules hold
//! it (`pushglobalfuncname`) joins the module's and the field's names.

use super::Ar;
use super::levels::tail_ar;
use super::traceback::{TbLine, traceback_plan};
use crate::runtime::puc_fmt::FmtArg;
use crate::runtime::{Coro, Gc, LuaStr, Table, Value};
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::puc_buffer::PucBuffer;

mod funcname;

impl Vm {
    /// The levels of `co`'s stack a traceback from `level` shows.
    fn traceback_ars(&mut self, co: Option<Gc<Coro>>, level: i64) -> Vec<(TbLine, Option<Ar>)> {
        let v = self.version();
        let ts = self.thread_stack(co);
        let plan = traceback_plan(v, ts.levels.len() as i64, level);
        plan.into_iter()
            .map(|line| {
                let ar = match line {
                    TbLine::Level(l) if l < 0 => Some(tail_ar()),
                    TbLine::Level(l) => Some(self.level_ar(&ts, l as usize)),
                    _ => None,
                };
                (line, ar)
            })
            .collect()
    }

    /// 5.1 `db_errorfb` after its arguments: `stack` holds the ones left
    /// (the message first, if any); the result is everything joined. The
    /// pieces are kept where PUC's C function keeps them, on its stack.
    pub(crate) fn puc_errorfb(
        &mut self,
        co: Option<Gc<Coro>>,
        stack: Vec<Value>,
        level: i64,
    ) -> Result<Value, LuaError> {
        let base = self.lib_roots.len();
        self.lib_roots.extend_from_slice(&stack);
        let r = self.errorfb_pieces(co, stack.is_empty(), level, base);
        let v = self.lib_roots.get(base).copied();
        self.lib_roots.truncate(base);
        r.map(|()| v.unwrap_or(Value::Nil))
    }

    fn errorfb_pieces(
        &mut self,
        co: Option<Gc<Coro>>,
        no_msg: bool,
        level: i64,
        base: usize,
    ) -> Result<(), LuaError> {
        let head: &[u8] = if no_msg { b"" } else { b"\n" };
        self.api_push(|h| h.try_intern(head))?;
        self.api_push(|h| h.try_intern(b"stack traceback:"))?;
        for (_, ar) in self.traceback_ars(co, level) {
            let Some(ar) = ar else {
                self.api_push(|h| h.try_intern(b"\n\t..."))?;
                continue;
            };
            self.api_push(|h| h.try_intern(b"\n\t"))?;
            self.api_push(|h| h.pushfstring(b"%s:", &[FmtArg::S(&ar.short_src)]))?;
            if ar.currentline > 0 {
                self.api_push(|h| h.pushfstring(b"%d:", &[FmtArg::D(ar.currentline)]))?;
            }
            match &ar.name {
                Some((what, n)) if !what.is_empty() => self.api_push(|h| {
                    h.pushfstring(b" in function '%s'", &[FmtArg::S(n.as_bytes())])
                })?,
                _ if ar.what == "main" => {
                    self.api_push(|h| h.pushfstring(b" in main chunk", &[]))?
                }
                _ if matches!(ar.what, "C" | "tail") => self.api_push(|h| h.try_intern(b" ?"))?,
                _ => self.api_push(|h| {
                    let args = [FmtArg::S(&ar.short_src), FmtArg::D(ar.linedefined)];
                    h.pushfstring(b" in function <%s:%d>", &args)
                })?,
            }
            self.api_concat(base)?;
        }
        self.api_concat(base)
    }

    /// A string an API call makes, pushed on the running C function's
    /// stack (`lib_roots`) and checked for as PUC checks.
    fn api_push(
        &mut self,
        f: impl FnOnce(&mut crate::runtime::Heap) -> Result<Gc<LuaStr>, crate::runtime::mem::Oom>,
    ) -> Result<(), LuaError> {
        self.api_check_before()?;
        let s = f(&mut self.heap)?;
        self.lib_roots.push(Value::Str(s));
        self.api_check_after_rooted()?;
        Ok(())
    }

    /// `lua_concat` of the values pushed from `base` up, which refuses the
    /// rightmost one that is no string after joining the ones above it.
    fn api_concat(&mut self, base: usize) -> Result<(), LuaError> {
        let n = self.lib_roots.len() - base;
        let text = |v: &Value| matches!(v, Value::Str(_) | Value::Int(_) | Value::Float(_));
        let bad = self.lib_roots[base..].iter().rposition(|v| !text(v));
        if n >= 2 && self.version() <= LuaVersion::Lua52 {
            self.native_checkpoint()?;
        }
        let Some(i) = bad else {
            self.concat_roots(n)?;
            if self.version() >= LuaVersion::Lua53 {
                self.native_checkpoint()?;
            }
            return Ok(());
        };
        self.concat_roots(n - i - 1)?;
        let bad = self.lib_roots[base + i].type_name();
        Err(self.plain_err(&format!("attempt to concatenate a {bad} value")))
    }

    /// `luaL_traceback(L, L, msg, level)` (5.2 on) as the standalone
    /// interpreter's message handler calls it: the string PUC makes.
    #[doc(hidden)]
    pub fn traceback_string(&mut self, msg: Option<&[u8]>, level: i64) -> Result<Value, LuaError> {
        Ok(Value::Str(self.puc_traceback(None, msg, level)?))
    }

    /// `luaL_traceback(L, L1, msg, level)` of 5.2 to 5.5, `L1` being `co`.
    pub(crate) fn puc_traceback(
        &mut self,
        co: Option<Gc<Coro>>,
        msg: Option<&[u8]>,
        level: i64,
    ) -> Result<Gc<LuaStr>, LuaError> {
        let ars = self.traceback_ars(co, level);
        if self.version() >= LuaVersion::Lua54 {
            return self.traceback_buffer(msg, ars);
        }
        let base = self.lib_roots.len();
        let r = self.traceback_pieces(msg, ars, base);
        let v = self.lib_roots.get(base).copied();
        self.lib_roots.truncate(base);
        r?;
        match v {
            Some(Value::Str(s)) => Ok(s),
            _ => unreachable!("a concatenation of strings"),
        }
    }

    /// 5.2 and 5.3: each level's pieces pushed and joined to what came
    /// before.
    fn traceback_pieces(
        &mut self,
        msg: Option<&[u8]>,
        ars: Vec<(TbLine, Option<Ar>)>,
        base: usize,
    ) -> Result<(), LuaError> {
        if let Some(m) = msg {
            self.api_push(|h| h.pushfstring(b"%s\n", &[FmtArg::S(m)]))?;
        }
        self.api_push(|h| h.try_intern(b"stack traceback:"))?;
        for (_, ar) in ars {
            let Some(ar) = ar else {
                self.api_push(|h| h.try_intern(b"\n\t..."))?;
                continue;
            };
            self.api_push(|h| h.pushfstring(b"\n\t%s:", &[FmtArg::S(&ar.short_src)]))?;
            if ar.currentline > 0 {
                self.api_push(|h| h.pushfstring(b"%d:", &[FmtArg::D(ar.currentline)]))?;
            }
            self.api_push(|h| h.try_intern(b" in "))?;
            let name = self.puc_funcname(&ar)?;
            self.lib_roots.push(Value::Str(name));
            if ar.istailcall {
                self.api_push(|h| h.try_intern(b"\n\t(...tail calls...)"))?;
            }
            self.api_concat(base)?;
        }
        self.api_concat(base)
    }

    /// 5.4 and 5.5: the traceback in a `luaL_Buffer`, each piece pushed
    /// (and checked for) before the buffer takes it.
    fn traceback_buffer(
        &mut self,
        msg: Option<&[u8]>,
        ars: Vec<(TbLine, Option<Ar>)>,
    ) -> Result<Gc<LuaStr>, LuaError> {
        let mut out = Vec::new();
        let mut b = PucBuffer::new(self);
        if let Some(m) = msg {
            out.extend_from_slice(m);
            b.add(self, &out, m.len())?;
            out.push(b'\n');
            b.add_chars(self, &out, 1)?;
        }
        out.extend_from_slice(b"stack traceback:");
        b.add(self, &out, 16)?;
        for (line, ar) in ars {
            match (line, ar) {
                (TbLine::Skip(n), _) => {
                    let args = [FmtArg::D(n)];
                    self.api_push(|h| h.pushfstring(b"\n\t...\t(skipping %d levels)", &args))?;
                    self.add_pushed(&mut b, &mut out)?;
                }
                (_, Some(ar)) => {
                    if ar.currentline <= 0 {
                        self.api_push(|h| {
                            h.pushfstring(b"\n\t%s: in ", &[FmtArg::S(&ar.short_src)])
                        })?;
                    } else {
                        let args = [FmtArg::S(&ar.short_src), FmtArg::D(ar.currentline)];
                        self.api_push(|h| h.pushfstring(b"\n\t%s:%d: in ", &args))?;
                    }
                    self.add_pushed(&mut b, &mut out)?;
                    let name = self.puc_funcname(&ar)?;
                    self.lib_roots.push(Value::Str(name));
                    self.add_pushed(&mut b, &mut out)?;
                    if ar.istailcall {
                        out.extend_from_slice(b"\n\t(...tail calls...)");
                        b.add(self, &out, 20)?;
                    }
                }
                _ => {}
            }
        }
        b.finish(self, &out)
    }

    /// `luaL_addvalue` of the string on top of `lib_roots`, popped after.
    fn add_pushed(&mut self, b: &mut PucBuffer, out: &mut Vec<u8>) -> Result<(), LuaError> {
        let Some(Value::Str(s)) = self.lib_roots.last().copied() else {
            unreachable!("a string pushed")
        };
        out.extend_from_slice(s.as_bytes());
        let r = b.add_value(self, out, s);
        self.lib_roots.pop();
        r
    }
}
