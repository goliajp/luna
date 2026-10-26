//! Which levels a traceback prints, as each version elides them.

use super::TbLine;
use crate::version::LuaVersion;

/// The lines `luaL_traceback` (5.1: `db_errorfb`) prints for a stack of `n`
/// levels starting at `level`, replaying each version's elision loop.
pub(in crate::vm::callstack) fn traceback_plan(v: LuaVersion, n: i64, level: i64) -> Vec<TbLine> {
    // 5.1's `lua_getstack` answers a negative level with a lost tail call
    let valid = |l: i64| l < n && (l >= 0 || v == LuaVersion::Lua51);
    // PUC `lastlevel` / `countlevels`: the deepest valid level, at least 0
    let last = (n - 1).max(0);
    let mut out = Vec::new();
    match v {
        LuaVersion::Lua51 => plan_51(&valid, level, &mut out),
        LuaVersion::Lua52 => plan_52(&valid, last, level, &mut out),
        _ => plan_53(v, &valid, last, level, &mut out),
    }
    out
}

/// 5.1's `db_errorfb`: the first 12 levels, "...", the last 10.
fn plan_51(valid: &impl Fn(i64) -> bool, mut level: i64, out: &mut Vec<TbLine>) {
    const LEVELS1: i64 = 12;
    const LEVELS2: i64 = 10;
    let mut firstpart = true;
    if level < 0 && valid(level) {
        out.push(TbLine::Tails(level.unsigned_abs()));
        level = 0;
    }
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

/// 5.2's `luaL_traceback`: the first 12 levels, "...", the last 10.
fn plan_52(valid: &impl Fn(i64) -> bool, last: i64, mut level: i64, out: &mut Vec<TbLine>) {
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

/// 5.3 on: the first 10 levels, "..." (5.4 on: how many it skips), the
/// last 11.
fn plan_53(
    v: LuaVersion,
    valid: &impl Fn(i64) -> bool,
    last: i64,
    mut level: i64,
    out: &mut Vec<TbLine>,
) {
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
