//! Pattern-based string functions: find, match, gmatch and gsub on top of
//! src/pattern.rs.

use super::posrelat;
use crate::pattern::{self, CapValue, Flavor, MatchState, PatError};
use crate::runtime::{Gc, LuaStr, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
mod gsub;
pub(crate) use gsub::*;

fn flavor(v: LuaVersion) -> Flavor {
    match v {
        LuaVersion::Lua51 => Flavor::Lua51,
        LuaVersion::Lua52 => Flavor::Lua52,
        _ => Flavor::Lua53,
    }
}

/// 5.1 reads patterns as C strings: a zero byte ends them.
fn pattern_bytes(v: LuaVersion, p: &[u8]) -> &[u8] {
    match (v, p.iter().position(|&b| b == 0)) {
        (LuaVersion::Lua51, Some(z)) => &p[..z],
        _ => p,
    }
}

fn pat_err(vm: &mut Vm, e: PatError) -> LuaError {
    raise_str(vm, &e.0)
}

fn cap_value(vm: &mut Vm, src: &[u8], c: CapValue) -> Value {
    match c {
        CapValue::Span(a, b) => Value::Str(vm.heap.intern(&src[a..b])),
        CapValue::Pos(p) => Value::Int(p as i64 + 1),
    }
}

/// PUC `push_captures`: every capture, or the whole match `[s, e)` when
/// there are none and `whole` is set.
fn push_captures(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    s: usize,
    e: usize,
    whole: bool,
    out: &mut Vec<Value>,
) -> Result<(), LuaError> {
    let n = if ms.level() == 0 && whole {
        1
    } else {
        ms.level()
    };
    for i in 0..n {
        let c = match ms.get_capture(i, s, e) {
            Ok(c) => c,
            Err(err) => {
                // over what was pushed before it
                vm.native_push(out.len() as u32);
                return Err(pat_err(vm, err));
            }
        };
        out.push(cap_value(vm, src, c));
    }
    Ok(())
}

/// PUC `nospecials`: no byte from `SPECIALS` (5.1 stops looking at a zero).
fn no_specials(v: LuaVersion, p: &[u8]) -> bool {
    !pattern::has_specials(pattern_bytes(v, p))
}

fn find_aux(vm: &mut Vm, fs: u32, nargs: u32, find: bool) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let s = argcheck::check_string(vm, a, 0)?;
    let p = argcheck::check_string(vm, a, 1)?;
    let (src, pat) = (s.as_bytes(), p.as_bytes());
    let ls = src.len();
    let init = posrelat(argcheck::opt_integer(vm, a, 2, 1)?, ls);
    let init = if v == LuaVersion::Lua51 {
        // 5.1 clamps a start past the end instead of failing
        (init - 1).clamp(0, ls as i64) as usize
    } else if init > ls as i64 + 1 {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    } else {
        (init.max(1) - 1) as usize
    };
    if find && (a.get(vm, 3).truthy() || no_specials(v, pat)) {
        return Ok(match pattern::plain_find(src, pat, init) {
            Some(at) => {
                let r = [
                    Value::Int(at as i64 + 1),
                    Value::Int((at + pat.len()) as i64),
                ];
                vm.nat_return(fs, &r)
            }
            None => vm.nat_return(fs, &[Value::Nil]),
        });
    }
    let pat = pattern_bytes(v, pat);
    let (anchor, body) = pattern::anchor_split(pat);
    let mut ms = MatchState::new(src, body, flavor(v));
    let mut s1 = init;
    loop {
        if let Some(e) = ms.try_at(s1).map_err(|err| pat_err(vm, err))? {
            let mut out = Vec::new();
            if find {
                out.push(Value::Int(s1 as i64 + 1));
                out.push(Value::Int(e as i64));
            }
            push_captures(vm, &ms, src, s1, e, !find, &mut out)?;
            return Ok(vm.nat_return(fs, &out));
        }
        if anchor || s1 >= ls {
            return Ok(vm.nat_return(fs, &[Value::Nil]));
        }
        s1 += 1;
    }
}

pub(super) fn s_find(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    find_aux(vm, fs, nargs, true)
}

pub(super) fn s_match(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    find_aux(vm, fs, nargs, false)
}

/// gmatch iterator; upvalues [subject, pattern, next start, end of the last
/// match or -1]. A '^' in the pattern is an ordinary character here.
///
/// Before 5.3 an empty match simply moves the next start one byte on; 5.3
/// instead rejects a match ending where the previous one ended.
fn gmatch_iter(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let (Value::Str(s), Value::Str(p), Value::Int(pos), Value::Int(last)) = (
        vm.nat_upval(fs, 0),
        vm.nat_upval(fs, 1),
        vm.nat_upval(fs, 2),
        vm.nat_upval(fs, 3),
    ) else {
        unreachable!("gmatch state")
    };
    let v = vm.version();
    let src = s.as_bytes();
    let mut ms = MatchState::new(src, pattern_bytes(v, p.as_bytes()), flavor(v));
    let legacy = v <= LuaVersion::Lua52;
    let mut from = pos as usize;
    while from <= src.len() {
        let m = ms.try_at(from).map_err(|err| pat_err(vm, err))?;
        if let Some(e) = m
            && (legacy || last != e as i64)
        {
            let next = if legacy && e == from { e + 1 } else { e };
            vm.nat_set_upval(fs, 2, Value::Int(next as i64));
            vm.nat_set_upval(fs, 3, Value::Int(e as i64));
            let mut out = Vec::new();
            push_captures(vm, &ms, src, from, e, true, &mut out)?;
            return Ok(vm.nat_return(fs, &out));
        }
        from += 1;
    }
    Ok(0)
}

pub(super) fn s_gmatch(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let s = argcheck::check_string(vm, a, 0)?;
    let p = argcheck::check_string(vm, a, 1)?;
    // the start position arrived in 5.4
    let init = if vm.version() >= LuaVersion::Lua54 {
        let ls = s.len() as i64;
        let i = posrelat(argcheck::opt_integer(vm, a, 2, 1)?, s.len()).max(1) - 1;
        i.min(ls + 1)
    } else {
        0
    };
    let it = vm.native_with(
        gmatch_iter,
        Box::new([
            Value::Str(s),
            Value::Str(p),
            Value::Int(init),
            Value::Int(-1),
        ]),
    );
    Ok(vm.nat_return(fs, &[it]))
}
