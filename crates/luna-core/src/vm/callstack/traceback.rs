//! `luaL_traceback` over the interleaved call stack.

use crate::runtime::{Coro, Gc, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

use super::Ar;
use super::levels::tail_ar;

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
            // 5.1's lost tail call at a negative level
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
