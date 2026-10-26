//! `luaL_traceback` over the interleaved call stack.

use crate::runtime::{Coro, Gc, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

use super::Ar;
use super::levels::tail_ar;

mod plan;
pub(super) use plan::traceback_plan;

/// One line of a traceback: a level, or where levels were left out.
pub(super) enum TbLine {
    Level(i64),
    /// 5.1: that many lost tail calls, the levels from a negative one up
    /// to 0 (as many as a level made from a huge number asks for)
    Tails(u64),
    /// `\n\t...` (5.1-5.3)
    Dots,
    /// `\n\t...\t(skipping N levels)` (5.4+)
    Skip(i64),
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
                    TbLine::Tails(_) => Some(tail_ar()),
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
                TbLine::Tails(k) => {
                    let ar = ars.next().expect("one Ar per level line");
                    for _ in 0..k {
                        self.traceback_line(&ar, &mut out, &mut names);
                    }
                }
            }
        }
        out
    }

    /// The running thread's levels, as `lua_getinfo` fills them.
    pub(crate) fn level_ars(&self) -> Vec<Ar> {
        let ts = self.thread_stack(None);
        (0..ts.levels.len())
            .map(|i| self.level_ar(&ts, i))
            .collect()
    }

    /// The traceback line of each of `ars`.
    pub(crate) fn lines_of(&mut self, ars: &[Ar]) -> Vec<Vec<u8>> {
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
/// `level_lines`, as if `hidden` more levels sat above the snapshot's level
/// 0: a message handler that prints a traceback runs on top of the stack
/// that raised, and 5.1 and 5.2 decide where to elide by absolute level
/// number, so those levels shift the elision.
pub(crate) fn traceback_from_lines<L: AsRef<[u8]>>(
    v: LuaVersion,
    lines: &[L],
    level: i64,
    hidden: i64,
) -> Vec<u8> {
    let mut out = Vec::new();
    for line in traceback_plan(v, lines.len() as i64 + hidden, level + hidden) {
        match line {
            TbLine::Dots => out.extend_from_slice(b"\n\t..."),
            TbLine::Skip(n) => {
                out.extend_from_slice(format!("\n\t...\t(skipping {n} levels)").as_bytes())
            }
            // 5.1's lost tail calls at negative levels
            TbLine::Tails(k) => {
                for _ in 0..k {
                    out.extend_from_slice(b"\n\t(tail call): ?");
                }
            }
            TbLine::Level(l) if l < 0 => out.extend_from_slice(b"\n\t(tail call): ?"),
            TbLine::Level(l) => out.extend_from_slice(lines[(l - hidden) as usize].as_ref()),
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
