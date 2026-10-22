//! Pattern-based string functions: find, match, gmatch and gsub on top of
//! src/pattern.rs.

use super::{posrelat, start_pos};
use crate::pattern::{self, CapValue, Flavor, MatchState, PatError};
use crate::runtime::{Gc, LuaStr, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::arg_error;
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
    // PUC's formats: each '%' shown is a `%%`, and the capture index is
    // `"invalid capture index %%%d"`
    use crate::runtime::puc::fmt::FmtArg;
    let conv = e.0.matches('%').count() + usize::from(e.0.starts_with("invalid capture index %"));
    if let Some(n) =
        e.0.strip_prefix("invalid capture index %")
            .and_then(|d| d.parse::<i64>().ok())
    {
        let fmt = b"invalid capture index %%%d";
        return crate::vm::builtins::raise_lib(vm, fmt, &[FmtArg::D(n)], conv as u32);
    }
    // the format of a literal message: each '%' a "%%"
    let fmt = e.0.replace('%', "%%");
    crate::vm::builtins::raise_lib(vm, fmt.as_bytes(), &[], conv as u32)
}

fn cap_value(vm: &mut Vm, src: &[u8], c: CapValue) -> Result<Value, LuaError> {
    match c {
        CapValue::Span(a, b) => vm.api_push_string(&src[a..b]),
        CapValue::Pos(p) => Ok(Value::Int(p as i64 + 1)),
    }
}

/// PUC `push_captures`: every capture, or the whole match `[s, e)` when
/// there are none and `whole` is set. `pushed` values are on the stack
/// above the native's arguments already; `out` holds those of them it
/// returns.
fn push_captures(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    (s, e): (usize, usize),
    whole: bool,
    pushed: u32,
    out: &mut Vec<Value>,
) -> Result<(), LuaError> {
    let n = if ms.level() == 0 && whole {
        1
    } else {
        ms.level()
    };
    let top = vm.native_top().expect("a native runs") + pushed;
    crate::vm::argcheck::check_stack_at(vm, top, n as i64, "too many captures")?;
    // what is pushed stays rooted while the next is made, which may
    // collect
    let base = vm.lib_roots.mark();
    vm.lib_roots.extend_from_slice(out);
    let r = push_each(vm, ms, src, (s, e, n), out);
    vm.lib_roots.release(base);
    r
}

fn push_each(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    (s, e, n): (usize, usize, usize),
    out: &mut Vec<Value>,
) -> Result<(), LuaError> {
    for i in 0..n {
        let c = match ms.get_capture(i, s, e) {
            Ok(c) => c,
            Err(err) => {
                // over what was pushed before it
                vm.native_push(out.len() as u32);
                return Err(pat_err(vm, err));
            }
        };
        let v = cap_value(vm, src, c)?;
        vm.lib_roots.push(v);
        out.push(v);
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
    let init = posrelat(start_pos(v, argcheck::opt_integer(vm, a, 2, 1)?), ls);
    let init = if v == LuaVersion::Lua51 {
        // 5.1 clamps a start past the end instead of failing
        (init - 1).clamp(0, ls as i64) as usize
    } else if init > ls as i64 + 1 {
        return vm.nat_ret(fs, &[Value::Nil]);
    } else {
        (init.max(1) - 1) as usize
    };
    if find && (a.get(vm, 3).truthy() || no_specials(v, pat)) {
        return match pattern::plain_find(src, pat, init) {
            Some(at) => {
                let r = [
                    Value::Int(at as i64 + 1),
                    Value::Int((at + pat.len()) as i64),
                ];
                vm.nat_ret(fs, &r)
            }
            None => vm.nat_ret(fs, &[Value::Nil]),
        };
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
            push_captures(vm, &ms, src, (s1, e), !find, out.len() as u32, &mut out)?;
            return vm.nat_ret(fs, &out);
        }
        if anchor || s1 >= ls {
            return vm.nat_ret(fs, &[Value::Nil]);
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
            push_captures(vm, &ms, src, (from, e), true, 0, &mut out)?;
            return vm.nat_ret(fs, &out);
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
        let i = argcheck::opt_integer(vm, a, 2, 1)?;
        let i = posrelat(start_pos(LuaVersion::Lua54, i), s.len()).max(1) - 1;
        i.min(ls + 1)
    } else {
        0
    };
    let ups = [
        Value::Str(s),
        Value::Str(p),
        Value::Int(init),
        Value::Int(-1),
    ];
    // PUC's iterator is a C closure of three upvalues, from 5.3 over a
    // `GMatchState` userdata: the userdata, its check for a collection,
    // then the closure (`lua_pushcclosure`) and its check
    let it = if vm.version() >= LuaVersion::Lua53 {
        use crate::runtime::heap::PucKind;
        let sz = vm.heap.sizes;
        let state = 71 * sz.ptr + 8;
        let objs = [
            (PucKind::Userdata, sz.userdata(0, state)),
            (PucKind::CClosure, sz.c_closure(3)),
        ];
        vm.heap.acct_obj(objs[0].0, objs[0].1);
        vm.api_check_after_rooted()?;
        let it = vm.lib_native_stand_in(gmatch_iter, &ups, &objs, 1)?;
        vm.api_check_after(it)?;
        it
    } else {
        vm.lib_native_puc(gmatch_iter, &ups, 3)?
    };
    vm.nat_ret(fs, &[it])
}
