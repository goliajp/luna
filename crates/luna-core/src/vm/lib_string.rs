//! string library: byte-string functions, the pattern-based family
//! (find/match/gmatch/gsub) on top of src/pattern.rs, and the shared string
//! metatable — method syntax on every dialect, arithmetic from 5.4.

use crate::numeric::{self, Num};
use crate::pattern::{self, CapValue, Flavor, MatchState, PatError};
use crate::runtime::{Gc, LuaStr, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::{ArithOp, Mm, Vm, arith_num};

type NativeFn = fn(&mut Vm, u32, u32) -> Result<u32, LuaError>;

pub(crate) fn open_string(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let v = vm.version();
    let set = |vm: &mut Vm, t: Gc<crate::runtime::Table>, name: &str, fv: Value| {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    };
    let mut fns: Vec<(&str, NativeFn)> = vec![
        ("len", s_len),
        ("sub", s_sub),
        ("upper", s_upper),
        ("lower", s_lower),
        ("rep", s_rep),
        ("reverse", s_reverse),
        ("byte", s_byte),
        ("char", s_char),
        ("find", s_find),
        ("match", s_match),
        ("gsub", s_gsub),
        ("format", crate::vm::lib_strformat::s_format),
        ("dump", s_dump),
    ];
    if v >= LuaVersion::Lua53 {
        fns.push(("pack", crate::vm::lib_strpack::s_pack));
        fns.push(("unpack", crate::vm::lib_strpack::s_unpack));
        fns.push(("packsize", crate::vm::lib_strpack::s_packsize));
    }
    for (name, f) in fns {
        let fv = vm.native(f);
        set(vm, t, name, fv);
    }
    // 5.1's LUA_COMPAT_GFIND keeps `gfind` as the very same function as
    // `gmatch`; the suite identity-tests them.
    let gmatch_v = vm.native(s_gmatch);
    set(vm, t, "gmatch", gmatch_v);
    if v == LuaVersion::Lua51 {
        set(vm, t, "gfind", gmatch_v);
    }
    vm.set_global("string", Value::Table(t))
        .expect("stdlib registration");
    vm.barrier_back_table(t);
    let mt = vm.heap.new_table();
    set(vm, mt, "__index", Value::Table(t));
    if v >= LuaVersion::Lua54 {
        let arith: [(&str, NativeFn); 8] = [
            ("__add", mm_add),
            ("__sub", mm_sub),
            ("__mul", mm_mul),
            ("__mod", mm_mod),
            ("__pow", mm_pow),
            ("__div", mm_div),
            ("__idiv", mm_idiv),
            ("__unm", mm_unm),
        ];
        for (name, f) in arith {
            let fv = vm.native(f);
            set(vm, mt, name, fv);
        }
    }
    vm.barrier_back_table(mt);
    vm.set_string_metatable(Some(mt));
}

/// PUC ≤5.3 `posrelat`: a negative position counts from the end, and one
/// before the start becomes 0. Callers clamp; 5.4's `posrelatI` /
/// `getendpos` come to the same after clamping.
fn posrelat(pos: i64, len: usize) -> i64 {
    if pos >= 0 {
        pos
    } else if pos.unsigned_abs() > len as u64 {
        0
    } else {
        len as i64 + pos + 1
    }
}

fn s_len(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    Ok(vm.nat_return(fs, &[Value::Int(s.len() as i64)]))
}

fn s_sub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let s = argcheck::check_string(vm, a, 0)?;
    let l = s.len();
    let start = posrelat(argcheck::check_integer(vm, a, 1)?, l).max(1);
    let end = posrelat(argcheck::opt_integer(vm, a, 2, -1)?, l).min(l as i64);
    let bytes: &[u8] = if start <= end {
        &s.as_bytes()[(start - 1) as usize..end as usize]
    } else {
        b""
    };
    let r = Value::Str(vm.heap.intern(bytes));
    Ok(vm.nat_return(fs, &[r]))
}

fn s_upper(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let out = s.as_bytes().to_ascii_uppercase();
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

fn s_lower(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let out = s.as_bytes().to_ascii_lowercase();
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

fn s_reverse(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let mut out = s.as_bytes().to_vec();
    out.reverse();
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

/// The longest string the library builds (1 GiB). Past each dialect's own
/// size check PUC goes on to ask the allocator; luna stops here and reports
/// what a failing allocator would.
pub(crate) const MAX_STR: u64 = 1 << 30;

fn s_rep(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let s = argcheck::check_string(vm, a, 0)?;
    let n = if v <= LuaVersion::Lua52 {
        i64::from(argcheck::check_int(vm, a, 1)?)
    } else {
        argcheck::check_integer(vm, a, 1)?
    };
    // 5.1 has no separator argument
    let sep = if v == LuaVersion::Lua51 {
        None
    } else {
        argcheck::opt_string(vm, a, 2)?
    };
    let (l, lsep) = (s.len() as u128, sep.map_or(0, |x| x.len()) as u128);
    // the result is empty for any count when both pieces are; PUC before
    // 5.5 spins `n` times producing it
    if n <= 0 || l + lsep == 0 {
        let r = Value::Str(vm.heap.intern(b""));
        return Ok(vm.nat_return(fs, &[r]));
    }
    let n = n as u128;
    // each dialect's own "too large" test; 5.1 has none
    let too_large = if v == LuaVersion::Lua51 {
        false
    } else if v == LuaVersion::Lua52 {
        l + lsep >= (usize::MAX >> 1) as u128 / n
    } else if v < LuaVersion::Lua55 {
        l + lsep > i32::MAX as u128 / n
    } else {
        l + lsep > i64::MAX as u128 / n
    };
    if too_large {
        return Err(raise_str(vm, "resulting string too large"));
    }
    let total = n * (l + lsep) - lsep;
    if total > u128::from(MAX_STR) {
        return Err(vm.plain_err("not enough memory"));
    }
    let mut out = Vec::with_capacity(total as usize);
    for k in 0..n {
        out.extend_from_slice(s.as_bytes());
        if let Some(sep) = sep
            && k + 1 < n
        {
            out.extend_from_slice(sep.as_bytes());
        }
    }
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

fn s_byte(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let s = argcheck::check_string(vm, a, 0)?;
    let l = s.len();
    let pi = posrelat(argcheck::opt_integer(vm, a, 1, 1)?, l);
    let pose = posrelat(argcheck::opt_integer(vm, a, 2, pi)?, l).min(l as i64);
    let posi = pi.max(1);
    if posi > pose {
        return Ok(0);
    }
    let bytes = &s.as_bytes()[(posi - 1) as usize..pose as usize];
    if let [b] = bytes {
        return Ok(vm.nat_return(fs, &[Value::Int(i64::from(*b))]));
    }
    argcheck::check_stack(vm, a, bytes.len() as i64, "string slice too long")?;
    let vals: Vec<Value> = bytes.iter().map(|&b| Value::Int(i64::from(b))).collect();
    Ok(vm.nat_return(fs, &vals))
}

fn s_char(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let mut out = Vec::with_capacity(nargs as usize);
    for i in 0..nargs {
        let c = if v <= LuaVersion::Lua52 {
            i64::from(argcheck::check_int(vm, a, i)?)
        } else {
            argcheck::check_integer(vm, a, i)?
        };
        if !(0..=255).contains(&c) {
            let msg = if v == LuaVersion::Lua51 {
                "invalid value"
            } else {
                "value out of range"
            };
            return Err(arg_error(vm, i + 1, msg));
        }
        out.push(c as u8);
    }
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

fn s_dump(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    // the strip flag arrived in 5.3
    let strip = v >= LuaVersion::Lua53 && a.get(vm, 1).truthy();
    let cl = if v >= LuaVersion::Lua55 {
        match a.get(vm, 0) {
            Value::Closure(cl) => cl,
            _ => return Err(arg_error(vm, 1, "Lua function expected")),
        }
    } else {
        match argcheck::check_function(vm, a, 0)? {
            Value::Closure(cl) => cl,
            _ => return Err(raise_str(vm, "unable to dump given function")),
        }
    };
    let bytes = crate::vm::dump::dump(&cl.proto, strip, v);
    let r = Value::Str(vm.heap.intern(&bytes));
    Ok(vm.nat_return(fs, &[r]))
}

// ---- pattern matching ----

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
        let c = ms.get_capture(i, s, e).map_err(|err| pat_err(vm, err))?;
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

fn s_find(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    find_aux(vm, fs, nargs, true)
}

fn s_match(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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

fn s_gmatch(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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

fn s_gsub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let s = argcheck::check_string(vm, a, 0)?;
    let p = argcheck::check_string(vm, a, 1)?;
    let repl = a.get(vm, 2);
    let srcl = s.len() as i64;
    let max_s: i128 = match v {
        LuaVersion::Lua51 => i128::from(argcheck::opt_int(vm, a, 3, (srcl + 1) as i32)?),
        // 5.2 keeps the count in a size_t: a negative one is huge
        LuaVersion::Lua52 => i128::from(argcheck::opt_integer(vm, a, 3, srcl + 1)? as u64),
        _ => i128::from(argcheck::opt_integer(vm, a, 3, srcl + 1)?),
    };
    let repl_ok = matches!(
        repl,
        Value::Str(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::Table(_)
            | Value::Closure(_)
            | Value::Native(_)
    );
    if !repl_ok {
        return Err(if v >= LuaVersion::Lua54 {
            argcheck::type_error(vm, a, 2, "string/function/table")
        } else {
            arg_error(vm, 3, "string/function/table expected")
        });
    }
    // a string or number replacement is a template
    let template = match repl {
        Value::Str(t) => Some(t),
        Value::Int(_) | Value::Float(_) => Some(argcheck::check_string(vm, a, 2)?),
        _ => None,
    };
    let src = s.as_bytes();
    let (anchor, body) = pattern::anchor_split(pattern_bytes(v, p.as_bytes()));
    let mut ms = MatchState::new(src, body, flavor(v));
    let mut out: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut n: i128 = 0;
    let mut last: Option<usize> = None;
    let mut changed = false;
    while n < max_s {
        let m = ms.try_at(pos).map_err(|err| pat_err(vm, err))?;
        // 5.3 rejects an empty match right after the previous match; earlier
        // versions take it and then copy a byte
        let m = match m {
            Some(e) if v >= LuaVersion::Lua53 && last == Some(e) => None,
            m => m,
        };
        if let Some(e) = m {
            n += 1;
            changed |= add_value(vm, &ms, src, pos, e, repl, template, &mut out)?;
            last = Some(e);
        }
        match m {
            Some(e) if v >= LuaVersion::Lua53 || e > pos => pos = e,
            _ if pos < src.len() => {
                out.push(src[pos]);
                pos += 1;
            }
            _ => break,
        }
        if anchor {
            break;
        }
    }
    // 5.4 hands back the subject itself when nothing was replaced
    let res = if v >= LuaVersion::Lua54 && !changed {
        Value::Str(s)
    } else {
        out.extend_from_slice(&src[pos..]);
        Value::Str(vm.heap.intern(&out))
    };
    Ok(vm.nat_return(fs, &[res, Value::Int(n as i64)]))
}

/// PUC `add_value`: append the replacement for the match `[s, e)`; false
/// when a function or table kept the original text.
#[allow(clippy::too_many_arguments)]
fn add_value(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    s: usize,
    e: usize,
    repl: Value,
    template: Option<Gc<LuaStr>>,
    out: &mut Vec<u8>,
) -> Result<bool, LuaError> {
    if let Some(t) = template {
        add_s(vm, ms, src, s, e, t.as_bytes(), out)?;
        return Ok(true);
    }
    let r = match repl {
        Value::Table(_) => {
            let k = ms.get_capture(0, s, e).map_err(|err| pat_err(vm, err))?;
            let k = cap_value(vm, src, k);
            vm.index_value(repl, k)?
        }
        f => {
            let mut args = Vec::new();
            push_captures(vm, ms, src, s, e, true, &mut args)?;
            // an unprotected C call: the replacement cannot yield
            vm.call_noyield(f, &args)?
                .first()
                .copied()
                .unwrap_or(Value::Nil)
        }
    };
    match r {
        Value::Nil | Value::Bool(false) => {
            out.extend_from_slice(&src[s..e]);
            Ok(false)
        }
        Value::Str(x) => {
            out.extend_from_slice(x.as_bytes());
            Ok(true)
        }
        n @ (Value::Int(_) | Value::Float(_)) => {
            let b = vm.tostring_basic(n);
            out.extend_from_slice(&b);
            Ok(true)
        }
        other => Err(raise_str(
            vm,
            &format!("invalid replacement value (a {})", other.type_name()),
        )),
    }
}

/// PUC `add_s`: expand `%0`-`%9` and `%%` in a template. 5.1 copies any
/// other escaped byte literally; later versions reject it.
fn add_s(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    s: usize,
    e: usize,
    t: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), LuaError> {
    let lenient = vm.version() == LuaVersion::Lua51;
    let mut i = 0;
    while i < t.len() {
        let c = t[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        // the template's terminating zero follows a final '%'
        let d = t.get(i).copied().unwrap_or(0);
        i += 1;
        match d {
            b'0' => out.extend_from_slice(&src[s..e]),
            b'1'..=b'9' => {
                let c = ms
                    .get_capture((d - b'1') as usize, s, e)
                    .map_err(|err| pat_err(vm, err))?;
                match c {
                    CapValue::Span(a, b) => out.extend_from_slice(&src[a..b]),
                    CapValue::Pos(p) => out.extend_from_slice((p + 1).to_string().as_bytes()),
                }
            }
            b'%' => out.push(b'%'),
            d if lenient => out.push(d),
            _ => return Err(raise_str(vm, "invalid use of '%' in replacement string")),
        }
    }
    Ok(())
}

// ---- string arithmetic metamethods (5.4+, lstrlib `stringmetamethods`) ----

/// lstrlib `tonum`: a number, or a string that converts as a whole.
fn tonum(v: Value) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        Value::Str(s) => numeric::str2num(s.as_bytes(), true, true),
        _ => None,
    }
}

/// lstrlib `arith`: both operands convertible → plain arithmetic, else the
/// second operand's metamethod (`trymt`) or an error naming both types.
fn string_arith(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    op: Option<ArithOp>,
    event: Mm,
    verb: &str,
) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = a.get(vm, 0);
    // `tonum` pushes the converted first operand, so with a single argument
    // that copy is what the second position then holds
    let y = match (a.is_none(1), tonum(x)) {
        (true, Some(Num::Int(i))) => Value::Int(i),
        (true, Some(Num::Float(f))) => Value::Float(f),
        _ => a.get(vm, 1),
    };
    if let (Some(nx), Some(ny)) = (tonum(x), tonum(y)) {
        let r = match op {
            Some(op) => arith_num(op, nx, ny).map_err(|msg| vm.plain_err(msg))?,
            // unary minus works on the second (duplicated) operand
            None => match ny {
                Num::Int(i) => Value::Int(i.wrapping_neg()),
                Num::Float(f) => Value::Float(-f),
            },
        };
        return Ok(vm.nat_return(fs, &[r]));
    }
    let mm = if matches!(y, Value::Str(_)) {
        Value::Nil
    } else {
        vm.get_mm(y, event)
    };
    if mm.is_nil() {
        let msg = format!(
            "attempt to {verb} a '{}' with a '{}'",
            x.type_name(),
            y.type_name()
        );
        return Err(raise_str(vm, &msg));
    }
    // lua_call from C: the metamethod cannot yield
    let r = vm
        .call_noyield(mm, &[x, y])?
        .first()
        .copied()
        .unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[r]))
}

fn mm_add(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Add), Mm::Add, "add")
}

fn mm_sub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Sub), Mm::Sub, "sub")
}

fn mm_mul(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Mul), Mm::Mul, "mul")
}

fn mm_mod(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Mod), Mm::Mod, "mod")
}

fn mm_pow(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Pow), Mm::Pow, "pow")
}

fn mm_div(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::Div), Mm::Div, "div")
}

fn mm_idiv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, Some(ArithOp::IDiv), Mm::IDiv, "idiv")
}

fn mm_unm(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    string_arith(vm, fs, nargs, None, Mm::Unm, "unm")
}
