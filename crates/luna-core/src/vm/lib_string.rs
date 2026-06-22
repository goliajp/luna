//! string library: core byte-string functions and the pattern-based family
//! (find/match/gmatch/gsub) on top of src/pattern.rs. Installs the shared
//! string metatable so `("x"):len()` method syntax works.

use crate::numeric::Num;
use crate::pattern::{self, Cap};
use crate::runtime::{Gc, LuaStr, Value};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_string(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }.set(&mut vm.heap, k, fv).expect("valid key");
    };
    set(vm, "len", s_len);
    set(vm, "sub", s_sub);
    set(vm, "upper", s_upper);
    set(vm, "lower", s_lower);
    set(vm, "rep", s_rep);
    set(vm, "reverse", s_reverse);
    set(vm, "byte", s_byte);
    set(vm, "char", s_char);
    set(vm, "find", s_find);
    set(vm, "match", s_match);
    // gmatch needs to be reused as 5.1's `gfind`; the suite identity-tests
    // them, so the *same* Value::Native has to land in both slots.
    let gmatch_v = vm.native(s_gmatch);
    let k = Value::Str(vm.heap.intern(b"gmatch"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }.set(&mut vm.heap, k, gmatch_v).expect("valid key");
    if vm.version() == crate::version::LuaVersion::Lua51 {
        let k = Value::Str(vm.heap.intern(b"gfind"));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }.set(&mut vm.heap, k, gmatch_v).expect("valid key");
    }
    set(vm, "gsub", s_gsub);
    set(vm, "format", s_format);
    set(vm, "dump", s_dump);
    // string.pack/unpack/packsize landed in 5.3 — 5.1/5.2 should not see them
    if vm.version() >= crate::version::LuaVersion::Lua53 {
        set(vm, "pack", crate::vm::lib_strpack::s_pack);
        set(vm, "unpack", crate::vm::lib_strpack::s_unpack);
        set(vm, "packsize", crate::vm::lib_strpack::s_packsize);
    }
    vm.set_global("string", Value::Table(t));
    vm.barrier_back_table(t);
    // shared string metatable: methods resolve through the library table
    let mt = vm.heap.new_table();
    let idx = Value::Str(vm.heap.intern(b"__index"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { mt.as_mut() }
        .set(&mut vm.heap, idx, Value::Table(t))
        .expect("valid key");
    vm.barrier_back_table(mt);
    vm.set_string_metatable(Some(mt));
}

pub(crate) fn check_str(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    i: u32,
    who: &str,
) -> Result<Gc<LuaStr>, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Str(s) => Ok(s),
        // numbers coerce to strings in string functions (PUC luaL_tolstring path)
        Value::Int(x) => {
            let s = crate::numeric::num_to_string(Num::Int(x));
            Ok(vm.heap.intern(s.as_bytes()))
        }
        Value::Float(x) => {
            let s = crate::numeric::num_to_string(Num::Float(x));
            Ok(vm.heap.intern(s.as_bytes()))
        }
        v => Err(arg_error(
            vm,
            i + 1,
            who,
            &format!("string expected, got {}", v.type_name()),
        )),
    }
}

/// PUC luaL_optinteger for a position argument: accepts integers, integral
/// floats, and numeric strings; otherwise a "bad argument #n to 'who'" error.
fn opt_int(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    i: u32,
    who: &str,
    default: i64,
) -> Result<i64, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Nil => Ok(default),
        Value::Int(x) => Ok(x),
        Value::Float(f) => crate::runtime::value::f2i_exact(f).ok_or_else(|| {
            arg_error(vm, i + 1, who, "number has no integer representation")
        }),
        Value::Str(s) => match crate::numeric::str2num(s.as_bytes(), true, true) {
            Some(Num::Int(x)) => Ok(x),
            Some(Num::Float(f)) => crate::runtime::value::f2i_exact(f).ok_or_else(|| {
                arg_error(vm, i + 1, who, "number has no integer representation")
            }),
            None => Err(arg_error(vm, i + 1, who, "number expected, got string")),
        },
        v => {
            let tn = vm.obj_typename(v);
            Err(arg_error(vm, i + 1, who, &format!("number expected, got {tn}")))
        }
    }
}

/// PUC posrelat: translate 1-based/negative positions.
fn posrelat(pos: i64, len: usize) -> i64 {
    if pos >= 0 {
        pos
    // i64::MIN's magnitude exceeds i64::MAX, so `(-pos) as usize` would
    // overflow in debug. Compare via the unsigned magnitude instead.
    } else if pos.unsigned_abs() as usize > len {
        0
    } else {
        len as i64 + pos + 1
    }
}

fn s_len(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "len")?;
    let n = s.len() as i64;
    Ok(vm.nat_return(fs, &[Value::Int(n)]))
}

fn s_dump(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // only Lua functions can be dumped (PUC str_dump); a strip flag drops the
    // debug names from the serialised chunk.
    let Value::Closure(cl) = vm.nat_arg(fs, nargs, 0) else {
        return Err(raise_str(vm, "unable to dump given function"));
    };
    let strip = vm.nat_arg(fs, nargs, 1).truthy();
    let bytes = crate::vm::dump::dump(&cl.proto, strip, vm.version());
    let v = Value::Str(vm.heap.intern(&bytes));
    Ok(vm.nat_return(fs, &[v]))
}

fn s_sub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "sub")?;
    let len = s.len();
    let mut i = posrelat(opt_int(vm, fs, nargs, 1, "sub", 1)?, len);
    let mut j = posrelat(opt_int(vm, fs, nargs, 2, "sub", -1)?, len);
    if i < 1 {
        i = 1;
    }
    if j > len as i64 {
        j = len as i64;
    }
    let out = if i > j {
        Vec::new()
    } else {
        s.as_bytes()[(i - 1) as usize..j as usize].to_vec()
    };
    let v = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[v]))
}

fn s_upper(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "upper")?;
    let out: Vec<u8> = s
        .as_bytes()
        .iter()
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let v = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[v]))
}

fn s_lower(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "lower")?;
    let out: Vec<u8> = s
        .as_bytes()
        .iter()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let v = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[v]))
}

const MAX_STR: usize = 1 << 30;

fn s_rep(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "rep")?;
    let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as a count")?;
    let sep: Vec<u8> = match vm.nat_arg(fs, nargs, 2) {
        Value::Nil => Vec::new(),
        Value::Str(x) => x.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                3,
                "rep",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    if n <= 0 {
        let v = Value::Str(vm.heap.intern(b""));
        return Ok(vm.nat_return(fs, &[v]));
    }
    let piece = s.len() + sep.len();
    if piece.saturating_mul(n as usize) > MAX_STR {
        return Err(raise_str(vm, "resulting string too large"));
    }
    let mut out = Vec::with_capacity(piece * n as usize);
    for k in 0..n {
        out.extend_from_slice(s.as_bytes());
        if k < n - 1 {
            out.extend_from_slice(&sep);
        }
    }
    let v = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[v]))
}

fn s_reverse(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "reverse")?;
    let mut out = s.as_bytes().to_vec();
    out.reverse();
    let v = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[v]))
}

fn s_byte(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "byte")?;
    let len = s.len();
    // PUC: clamp AFTER both translations; j defaults to the raw i position
    let pi = posrelat(opt_int(vm, fs, nargs, 1, "byte", 1)?, len);
    let pj = posrelat(opt_int(vm, fs, nargs, 2, "byte", pi)?, len);
    let i = pi.max(1);
    let j = pj.min(len as i64);
    if i > j {
        return Ok(0);
    }
    let vals: Vec<Value> = s.as_bytes()[(i - 1) as usize..j as usize]
        .iter()
        .map(|&b| Value::Int(b as i64))
        .collect();
    Ok(vm.nat_return(fs, &vals))
}

fn s_char(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let mut out = Vec::with_capacity(nargs as usize);
    for i in 0..nargs {
        let c = vm.int_from(vm.nat_arg(fs, nargs, i), "use as a character code")?;
        if !(0..=255).contains(&c) {
            return Err(arg_error(vm, i + 1, "char", "value out of range"));
        }
        out.push(c as u8);
    }
    let v = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[v]))
}

// ---- pattern-based functions ----

fn pat_err(vm: &mut Vm, e: pattern::PatError) -> LuaError {
    raise_str(vm, &e.0)
}

/// Captures → Lua values; an empty capture list yields the whole match.
fn push_captures(vm: &mut Vm, src: &[u8], m: &pattern::Match, out: &mut Vec<Value>) {
    if m.caps.is_empty() {
        let s = Value::Str(vm.heap.intern(&src[m.start..m.end]));
        out.push(s);
        return;
    }
    for &c in &m.caps {
        match c {
            Cap::Span(a, b) => {
                let s = Value::Str(vm.heap.intern(&src[a..b]));
                out.push(s);
            }
            Cap::Pos(p) => out.push(Value::Int(p as i64 + 1)),
        }
    }
}

/// Common init handling: 1-based, negative-from-end, clamped.
fn init_offset(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    arg: u32,
    who: &str,
    len: usize,
) -> Result<Option<usize>, LuaError> {
    let raw = posrelat(opt_int(vm, fs, nargs, arg, who, 1)?, len);
    if raw > len as i64 + 1 {
        return Ok(None); // past the end: no match possible
    }
    Ok(Some((raw.max(1) - 1) as usize))
}

fn s_find(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "find")?;
    let p = check_str(vm, fs, nargs, 1, "find")?;
    let src = s.as_bytes().to_vec();
    let pat = p.as_bytes().to_vec();
    let Some(init) = init_offset(vm, fs, nargs, 2, "find", src.len())? else {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    };
    let plain = vm.nat_arg(fs, nargs, 3).truthy();
    if plain || !pattern::has_specials(&pat) {
        return match pattern::plain_find(&src, &pat, init) {
            Some(at) => {
                let st = Value::Int(at as i64 + 1);
                let en = Value::Int((at + pat.len()) as i64);
                Ok(vm.nat_return(fs, &[st, en]))
            }
            None => Ok(vm.nat_return(fs, &[Value::Nil])),
        };
    }
    match pattern::find(&src, &pat, init).map_err(|e| pat_err(vm, e))? {
        Some(m) => {
            let mut out = vec![Value::Int(m.start as i64 + 1), Value::Int(m.end as i64)];
            if !m.caps.is_empty() {
                push_captures(vm, &src, &m, &mut out);
            }
            Ok(vm.nat_return(fs, &out))
        }
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

fn s_match(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "match")?;
    let p = check_str(vm, fs, nargs, 1, "match")?;
    let src = s.as_bytes().to_vec();
    let pat = p.as_bytes().to_vec();
    let Some(init) = init_offset(vm, fs, nargs, 2, "match", src.len())? else {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    };
    match pattern::find(&src, &pat, init).map_err(|e| pat_err(vm, e))? {
        Some(m) => {
            let mut out = Vec::new();
            push_captures(vm, &src, &m, &mut out);
            Ok(vm.nat_return(fs, &out))
        }
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

/// gmatch iterator: upvalues [src, pat, pos, lastmatch]. `lastmatch` is the
/// end of the previous match (-1 = none); PUC gmatch_aux rejects an empty
/// match whose end coincides with it, scanning one byte forward instead, so
/// `gmatch("ab", "()%s*()")` advances cleanly past empty matches.
fn gmatch_iter(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(s) = vm.nat_upval(fs, 0) else {
        unreachable!()
    };
    let Value::Str(p) = vm.nat_upval(fs, 1) else {
        unreachable!()
    };
    let Value::Int(pos) = vm.nat_upval(fs, 2) else {
        unreachable!()
    };
    let last = match vm.nat_upval(fs, 3) {
        Value::Int(x) if x >= 0 => Some(x as usize),
        _ => None,
    };
    let src = s.as_bytes().to_vec();
    let pat = p.as_bytes().to_vec();
    let (anchor, body) = pattern::anchor_split(&pat);
    let mut sp = pos as usize;
    while sp <= src.len() {
        if let Some(m) = pattern::match_at(&src, body, sp).map_err(|e| pat_err(vm, e))?
            && last != Some(m.end)
        {
            vm.nat_set_upval(fs, 2, Value::Int(m.end as i64));
            vm.nat_set_upval(fs, 3, Value::Int(m.end as i64));
            let mut out = Vec::new();
            push_captures(vm, &src, &m, &mut out);
            return Ok(vm.nat_return(fs, &out));
        }
        if anchor {
            break;
        }
        sp += 1;
    }
    Ok(vm.nat_return(fs, &[Value::Nil]))
}

fn s_gmatch(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "gmatch")?;
    let p = check_str(vm, fs, nargs, 1, "gmatch")?;
    // optional 1-based init (5.4): clamp; past the end means no iterations
    let init = match init_offset(vm, fs, nargs, 2, "gmatch", s.len())? {
        Some(off) => off as i64,
        None => s.len() as i64 + 1,
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
    let s = check_str(vm, fs, nargs, 0, "gsub")?;
    let p = check_str(vm, fs, nargs, 1, "gsub")?;
    let repl = vm.nat_arg(fs, nargs, 2);
    match repl {
        Value::Str(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Table(_)
        | Value::Closure(_)
        | Value::Native(_) => {}
        v => {
            return Err(arg_error(
                vm,
                3,
                "gsub",
                &format!("string/function/table expected, got {}", v.type_name()),
            ));
        }
    }
    let max_n = match vm.nat_arg(fs, nargs, 3) {
        Value::Nil => i64::MAX,
        v => vm.int_from(v, "use as a count")?,
    };
    let src = s.as_bytes().to_vec();
    let pat = p.as_bytes().to_vec();
    let (anchor, body) = pattern::anchor_split(&pat);
    let body = body.to_vec();
    let mut out: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut count: i64 = 0;
    // PUC str_gsub: match anchored at the current position; reject an empty
    // match whose end coincides with the previous match (so " *" over "a b"
    // yields "-a-b-", not "-a--b-"); otherwise copy one byte and advance.
    let mut last_match: Option<usize> = None;
    // PUC reuses the original string when nothing actually changed (no match,
    // or every function/table replacement returned nil/false). `count` still
    // counts matches; `changed` gates the reuse.
    let mut changed = false;
    while count < max_n {
        let m = pattern::match_at(&src, &body, pos).map_err(|e| pat_err(vm, e))?;
        match m {
            Some(m) if last_match != Some(m.end) => {
                count += 1;
                changed |= gsub_one(vm, &src, &m, repl, &mut out)?;
                pos = m.end;
                last_match = Some(m.end);
            }
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
    let res = if changed {
        out.extend_from_slice(&src[pos..]);
        Value::Str(vm.heap.intern(&out))
    } else {
        Value::Str(s)
    };
    Ok(vm.nat_return(fs, &[res, Value::Int(count)]))
}

/// One replacement (PUC add_value): string template, table lookup, or call.
fn gsub_one(
    vm: &mut Vm,
    src: &[u8],
    m: &pattern::Match,
    repl: Value,
    out: &mut Vec<u8>,
) -> Result<bool, LuaError> {
    let whole = &src[m.start..m.end];
    let cap_value = |vm: &mut Vm, idx: usize| -> Result<Value, LuaError> {
        if m.caps.is_empty() {
            if idx == 0 {
                return Ok(Value::Str(vm.heap.intern(whole)));
            }
            return Err(raise_str(
                vm,
                &format!("invalid capture index %{}", idx + 1),
            ));
        }
        match m.caps.get(idx) {
            Some(Cap::Span(a, b)) => Ok(Value::Str(vm.heap.intern(&src[*a..*b]))),
            Some(Cap::Pos(p)) => Ok(Value::Int(*p as i64 + 1)),
            None => Err(raise_str(
                vm,
                &format!("invalid capture index %{}", idx + 1),
            )),
        }
    };
    let result = match repl {
        Value::Str(r) => {
            let t = r.as_bytes().to_vec();
            let mut i = 0;
            while i < t.len() {
                if t[i] == b'%' {
                    i += 1;
                    match t.get(i) {
                        Some(b'%') => out.push(b'%'),
                        Some(&d @ b'0'..=b'9') => {
                            if d == b'0' {
                                out.extend_from_slice(whole);
                            } else {
                                let v = cap_value(vm, (d - b'1') as usize)?;
                                append_value(vm, v, out)?;
                            }
                        }
                        _ => {
                            return Err(raise_str(vm, "invalid use of '%' in replacement string"));
                        }
                    }
                    i += 1;
                } else {
                    out.push(t[i]);
                    i += 1;
                }
            }
            return Ok(true);
        }
        Value::Int(_) | Value::Float(_) => {
            let bytes = vm.tostring_basic(repl);
            out.extend_from_slice(&bytes);
            return Ok(true);
        }
        Value::Table(t) => {
            // PUC gsub uses lua_gettable: the lookup honours __index
            let k = cap_value(vm, 0)?;
            vm.index_value(Value::Table(t), k)?
        }
        f @ (Value::Closure(_) | Value::Native(_)) => {
            let mut args = Vec::new();
            push_captures(vm, src, m, &mut args);
            // gsub is an unprotected C call: the replacement runs non-yieldable.
            vm.call_noyield(f, &args)?
                .first()
                .copied()
                .unwrap_or(Value::Nil)
        }
        _ => unreachable!(),
    };
    match result {
        // function/table returning nil/false keeps the original text unchanged
        Value::Nil | Value::Bool(false) => {
            out.extend_from_slice(whole);
            Ok(false)
        }
        v => {
            append_value(vm, v, out)?;
            Ok(true)
        }
    }
}

fn append_value(vm: &mut Vm, v: Value, out: &mut Vec<u8>) -> Result<(), LuaError> {
    match v {
        Value::Str(s) => out.extend_from_slice(s.as_bytes()),
        Value::Int(_) | Value::Float(_) => {
            let b = vm.tostring_basic(v);
            out.extend_from_slice(&b);
        }
        v => {
            return Err(raise_str(
                vm,
                &format!("invalid replacement value (a {})", v.type_name()),
            ));
        }
    }
    Ok(())
}

// ---- string.format (C printf subset, PUC semantics) ----

struct Spec {
    minus: bool,
    plus: bool,
    space: bool,
    hash: bool,
    zero: bool,
    width: usize,
    prec: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum PadKind {
    Str,
    Int,
    Float,
}

fn pad(out: &mut Vec<u8>, body: Vec<u8>, spec: &Spec, kind: PadKind) {
    let w = spec.width;
    if body.len() >= w {
        out.extend_from_slice(&body);
        return;
    }
    let fill = w - body.len();
    // C: an explicit precision makes the '0' flag inert for integer conversions,
    // but NOT for floating conversions (precision there = fractional digits).
    let zero_pad = spec.zero
        && match kind {
            PadKind::Float => true,
            PadKind::Int => spec.prec.is_none(),
            PadKind::Str => false,
        };
    if spec.minus {
        out.extend_from_slice(&body);
        out.extend(std::iter::repeat_n(b' ', fill));
    } else if zero_pad {
        // zero padding goes after any sign/prefix
        let sign_len = body
            .iter()
            .take_while(|&&c| c == b'-' || c == b'+' || c == b' ')
            .count()
            + if body.len() > 1 && body[0] == b'0' && matches!(body.get(1), Some(b'x' | b'X')) {
                2
            } else {
                0
            };
        out.extend_from_slice(&body[..sign_len]);
        out.extend(std::iter::repeat_n(b'0', fill));
        out.extend_from_slice(&body[sign_len..]);
    } else {
        out.extend(std::iter::repeat_n(b' ', fill));
        out.extend_from_slice(&body);
    }
}

fn fmt_int(spec: &Spec, v: i64, base: u32, upper: bool, signed: bool) -> Vec<u8> {
    let (neg, mag) = if signed && v < 0 {
        (true, (v as i128).unsigned_abs())
    } else if signed {
        (false, v as u128)
    } else {
        (false, v as u64 as u128)
    };
    let mut digits: Vec<u8> = Vec::new();
    let mut m = mag;
    loop {
        let d = (m % base as u128) as u8;
        digits.push(if d < 10 {
            b'0' + d
        } else if upper {
            b'A' + d - 10
        } else {
            b'a' + d - 10
        });
        m /= base as u128;
        if m == 0 {
            break;
        }
    }
    digits.reverse();
    if let Some(p) = spec.prec {
        while digits.len() < p {
            digits.insert(0, b'0');
        }
        if p == 0 && mag == 0 {
            digits.clear();
        }
    }
    let mut body = Vec::new();
    if neg {
        body.push(b'-');
    } else if spec.plus && signed {
        body.push(b'+');
    } else if spec.space && signed {
        body.push(b' ');
    }
    if spec.hash && base == 16 && mag != 0 {
        body.extend_from_slice(if upper { b"0X" } else { b"0x" });
    }
    if spec.hash && base == 8 && digits.first() != Some(&b'0') {
        body.push(b'0');
    }
    body.extend_from_slice(&digits);
    body
}

/// C-style exponent form: d.dddddde±XX (at least two exponent digits).
fn fmt_exp(v: f64, prec: usize, upper: bool) -> Vec<u8> {
    let s = format!("{v:.prec$e}");
    // rust: "1.5e2" / "1.5e-7" → C: "1.5e+02" / "1.5e-07"
    let (mant, exp) = s.split_once('e').expect("exponent");
    let (sign, digits) = match exp.strip_prefix('-') {
        Some(d) => ('-', d),
        None => ('+', exp),
    };
    let e = if upper { 'E' } else { 'e' };
    format!("{mant}{e}{sign}{digits:0>2}").into_bytes()
}

fn fmt_g(v: f64, prec: usize, upper: bool, hash: bool) -> Vec<u8> {
    let p = prec.max(1);
    // decide notation from the decimal exponent at p significant digits
    let rounded = format!("{v:.*e}", p - 1);
    let exp: i32 = rounded
        .split_once('e')
        .map(|(_, e)| e.parse().unwrap_or(0))
        .unwrap_or(0);
    let mut body = if exp < -4 || exp >= p as i32 {
        fmt_exp(v, p - 1, upper)
    } else {
        let dec_prec = (p as i32 - 1 - exp).max(0) as usize;
        format!("{v:.dec_prec$}").into_bytes()
    };
    if !hash {
        // strip trailing zeros (and a trailing point) from the mantissa
        if let Some(dot) = body.iter().position(|&c| c == b'.') {
            let mant_end = body
                .iter()
                .position(|&c| c == b'e' || c == b'E')
                .unwrap_or(body.len());
            let mut last = mant_end;
            while last > dot + 1 && body[last - 1] == b'0' {
                last -= 1;
            }
            if last == dot + 1 {
                last = dot;
            }
            body.drain(last..mant_end);
        }
    }
    body
}

/// C %a hex-float form.
fn fmt_hex_float(v: f64, upper: bool, prec: Option<usize>) -> Vec<u8> {
    let bits = v.to_bits();
    let sign = if bits >> 63 != 0 { "-" } else { "" };
    let exp_bits = ((bits >> 52) & 0x7FF) as i64;
    let frac = bits & ((1u64 << 52) - 1);
    let s = if v.is_nan() {
        format!("{sign}nan")
    } else if v.is_infinite() {
        format!("{sign}inf")
    } else if exp_bits == 0 && frac == 0 {
        // signed zero: still honour an explicit precision (.N → N trailing 0s)
        match prec {
            Some(p) if p > 0 => format!("{sign}0x0.{}p+0", "0".repeat(p)),
            _ => format!("{sign}0x0p+0"),
        }
    } else {
        let (mut lead, exp, mant) = if exp_bits == 0 {
            (0u64, -1022i64, frac)
        } else {
            (1u64, exp_bits - 1023, frac)
        };
        let hex = match prec {
            // No precision: full mantissa with trailing zeros trimmed.
            None => {
                let mut h = format!("{mant:013x}");
                while h.len() > 1 && h.ends_with('0') {
                    h.pop();
                }
                if mant == 0 { String::new() } else { h }
            }
            // Explicit precision p: round the 13-hex-digit mantissa (52 bits)
            // to p hex digits, ties to even, carrying into `lead` on overflow.
            Some(p) => {
                if p >= 13 {
                    format!("{mant:013x}{}", "0".repeat(p - 13))
                } else {
                    let shift = 52 - 4 * p as u32;
                    let mut r = mant >> shift;
                    let low = mant & ((1u64 << shift) - 1);
                    let half = 1u64 << (shift - 1);
                    if low > half || (low == half && r & 1 == 1) {
                        r += 1;
                        if r >= 1u64 << (4 * p) {
                            r = 0;
                            lead += 1;
                        }
                    }
                    if p == 0 { String::new() } else { format!("{r:0width$x}", width = p) }
                }
            }
        };
        if hex.is_empty() {
            format!("{sign}0x{lead}p{exp:+}")
        } else {
            format!("{sign}0x{lead}.{hex}p{exp:+}")
        }
    };
    if upper {
        s.to_uppercase().into_bytes()
    } else {
        s.into_bytes()
    }
}

fn quote_value(vm: &mut Vm, v: Value, out: &mut Vec<u8>) -> Result<(), LuaError> {
    match v {
        Value::Str(s) => {
            out.push(b'"');
            let bytes = s.as_bytes().to_vec();
            for (i, &c) in bytes.iter().enumerate() {
                match c {
                    // PUC addquoted: `"`, `\`, and newline are backslash +
                    // the byte itself — so `\n` becomes backslash + a real
                    // newline (not `\n`), readable back as the same string.
                    b'"' | b'\\' | b'\n' => {
                        out.push(b'\\');
                        out.push(c);
                    }
                    // other control bytes (incl. \0 and \r) → `\d`, or `\ddd`
                    // when a digit follows so the run isn't misparsed. PUC 5.1
                    // formats `\0` specifically as `\000` (4 chars) regardless
                    // of the next byte — strings.lua 5.1 :105 bakes that in.
                    c if c.is_ascii_control() => {
                        let always_three =
                            c == 0 && vm.version() == crate::version::LuaVersion::Lua51;
                        if always_three
                            || bytes.get(i + 1).is_some_and(|d| d.is_ascii_digit())
                        {
                            out.extend_from_slice(format!("\\{c:03}").as_bytes());
                        } else {
                            out.extend_from_slice(format!("\\{c}").as_bytes());
                        }
                    }
                    c => out.push(c),
                }
            }
            out.push(b'"');
        }
        Value::Int(i) => {
            if i == i64::MIN {
                // LUA_MININTEGER: a decimal literal would reparse as a float
                // (the positive magnitude overflows); a hex literal wraps back
                // to the integer (PUC addliteral's corner case).
                out.extend_from_slice(format!("0x{:x}", i as u64).as_bytes());
            } else {
                out.extend_from_slice(i.to_string().as_bytes());
            }
        }
        Value::Float(f) => {
            if f.is_nan() {
                out.extend_from_slice(b"(0/0)");
            } else if f.is_infinite() {
                out.extend_from_slice(if f < 0.0 { b"-1e9999" } else { b"1e9999" });
            } else if f == f.floor() {
                // integral float: keep readability and the float subtype
                out.extend_from_slice(format!("{f:.1}").as_bytes());
            } else {
                out.extend_from_slice(&fmt_hex_float(f, false, None));
            }
        }
        Value::Nil => out.extend_from_slice(b"nil"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        v => {
            return Err(raise_str(
                vm,
                &format!("value has no literal form (a {} value)", v.type_name()),
            ));
        }
    }
    Ok(())
}

/// PUC MAX_FORMAT: cap on a single conversion spec's length.
const MAX_FORMAT: usize = 32;

/// Port of PUC `checkformat`: verify that a conversion spec uses only the flags
/// valid for that conversion (and a precision only where one is allowed). The
/// scanner already bounded width/precision to two digits each; anything left
/// over before the conversion letter makes the spec invalid.
fn checkformat(vm: &mut Vm, form: &[u8], flags: &[u8], precision: bool) -> Result<(), LuaError> {
    let mut k = 1; // skip '%'
    while form.get(k).is_some_and(|c| flags.contains(c)) {
        k += 1;
    }
    if form.get(k) != Some(&b'0') {
        // get2digits: skip up to two width digits
        if form.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
            if form.get(k).is_some_and(u8::is_ascii_digit) {
                k += 1;
            }
        }
        if form.get(k) == Some(&b'.') && precision {
            k += 1;
            if form.get(k).is_some_and(u8::is_ascii_digit) {
                k += 1;
                if form.get(k).is_some_and(u8::is_ascii_digit) {
                    k += 1;
                }
            }
        }
    }
    if !form.get(k).is_some_and(u8::is_ascii_alphabetic) {
        return Err(raise_str(
            vm,
            &format!(
                "invalid conversion specification: '{}'",
                String::from_utf8_lossy(form)
            ),
        ));
    }
    Ok(())
}

fn s_format(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = check_str(vm, fs, nargs, 0, "format")?;
    let fmt = f.as_bytes().to_vec();
    let mut out: Vec<u8> = Vec::new();
    let mut argi: u32 = 1; // next argument
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        if fmt.get(i) == Some(&b'%') {
            out.push(b'%');
            i += 1;
            continue;
        }
        // Scan the conversion spec the way PUC getformat does: the maximal run
        // of flag/width/precision bytes, then the conversion letter. `form`
        // keeps the raw spec (incl. '%' and conv) for checkformat diagnostics.
        // PUC ≤5.3 also rejects a repeated flag byte during this scan so
        // pathological inputs like `%000…000d` surface as "repeated flags"
        // before the spec-length cap fires.
        let spec_start = i;
        let legacy_repeated = vm.version() <= crate::version::LuaVersion::Lua53;
        let mut seen_flags: u32 = 0;
        // `in_flags` stays true while we are still inside the leading flag
        // run; once a non-flag digit (1-9) or a `.` is seen, subsequent `0`s
        // belong to width/precision and must not trip the repeated-flag check.
        let mut in_flags = true;
        while let Some(&c) = fmt.get(i) {
            if !b"-+#0 123456789.".contains(&c) {
                break;
            }
            if legacy_repeated && in_flags {
                let bit = match c {
                    b'-' => Some(1u32 << 0),
                    b'+' => Some(1u32 << 1),
                    b' ' => Some(1u32 << 2),
                    b'#' => Some(1u32 << 3),
                    b'0' => Some(1u32 << 4),
                    _ => None,
                };
                if let Some(bit) = bit {
                    if seen_flags & bit != 0 {
                        return Err(raise_str(vm, "invalid format (repeated flags)"));
                    }
                    seen_flags |= bit;
                }
            }
            // a non-zero digit or '.' switches us out of flag-scanning into
            // width/precision parsing — further digits are not flag bytes.
            if matches!(c, b'1'..=b'9' | b'.') {
                in_flags = false;
            }
            i += 1;
        }
        // getformat: the spec (run + conversion char) must fit MAX_FORMAT.
        if i - spec_start + 1 >= MAX_FORMAT - 10 {
            return Err(raise_str(vm, "invalid format (too long)"));
        }
        let mut form = vec![b'%'];
        form.extend_from_slice(&fmt[spec_start..i]);
        let Some(&conv) = fmt.get(i) else {
            return Err(raise_str(
                vm,
                &format!(
                    "invalid conversion '{}' to 'format'",
                    String::from_utf8_lossy(&form)
                ),
            ));
        };
        form.push(conv);
        i += 1;
        // Decode flags/width/precision for rendering. Specs that survive the
        // per-conversion checkformat have at most two-digit width/precision.
        let mut spec = Spec {
            minus: false,
            plus: false,
            space: false,
            hash: false,
            zero: false,
            width: 0,
            prec: None,
        };
        let mut k = spec_start;
        // PUC ≤5.3 `checkformat` rejects a repeated flag byte ("repeated
        // flags"); 5.4+ silently allows duplicates (the spec-length cap then
        // tames pathological inputs). strings.lua 5.2/5.3 :303 probes
        // `%000…000d` (600 zeros) and expects the early error.
        let legacy_flag_check = vm.version() <= crate::version::LuaVersion::Lua53;
        let mut seen = 0u32;
        while let Some(&c) = fmt.get(k) {
            let bit = match c {
                b'-' => 1u32 << 0,
                b'+' => 1u32 << 1,
                b' ' => 1u32 << 2,
                b'#' => 1u32 << 3,
                b'0' => 1u32 << 4,
                _ => break,
            };
            if legacy_flag_check && seen & bit != 0 {
                return Err(raise_str(vm, "invalid format (repeated flags)"));
            }
            seen |= bit;
            match c {
                b'-' => spec.minus = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.hash = true,
                b'0' => spec.zero = true,
                _ => break,
            }
            k += 1;
        }
        let mut wd = 0usize;
        while let Some(&c @ b'0'..=b'9') = fmt.get(k) {
            wd = wd * 10 + (c - b'0') as usize;
            k += 1;
        }
        spec.width = wd;
        if fmt.get(k) == Some(&b'.') {
            k += 1;
            let mut p = 0usize;
            while let Some(&c @ b'0'..=b'9') = fmt.get(k) {
                p = p * 10 + (c - b'0') as usize;
                k += 1;
            }
            spec.prec = Some(p);
        }
        // PUC ≤5.3 caps `width` and `precision` at < 100 each ("too long") —
        // lstrlib.c's `checkformat` enforces a per-field two-digit ceiling.
        // 5.4+ reports the same overflow as "invalid conversion" via the
        // spec-letter validation (luna's fallthrough path already emits
        // exactly that string). strings.lua 5.2/5.3 :298 needs "too long".
        if vm.version() <= crate::version::LuaVersion::Lua53
            && (spec.width >= 100 || spec.prec.unwrap_or(0) >= 100)
        {
            return Err(raise_str(vm, "invalid format (width or precision too long)"));
        }
        if argi >= nargs && conv != b'%' {
            return Err(arg_error(vm, argi + 1, "format", "no value"));
        }
        let arg = vm.nat_arg(fs, nargs, argi);
        argi += 1;
        // PUC ≤5.2 silently truncates a non-integer float for `%d`/`%i`/`%u`/
        // `%o`/`%x`/`%X`/`%c` (no integer subtype), while 5.3+ raises
        // "number has no integer representation". strings.lua 5.2 :151 probes
        // `string.format("%x", 0.3) == "0"`.
        let int_from_lenient = |vm: &mut Vm, arg: Value, signed: bool| -> Result<i64, LuaError> {
            // PUC 5.2 `%u`/`%x`/`%X`/`%o` rejects negative operands (lstrlib.c
            // casts through `unsigned int` after a non-negative check).
            // strings.lua :214 exercises this with `%x` on `-1`.
            if !signed && vm.version() <= crate::version::LuaVersion::Lua52 {
                let neg = match arg {
                    Value::Int(i) => i < 0,
                    Value::Float(f) => f < 0.0,
                    _ => false,
                };
                if neg {
                    return Err(raise_str(vm, "bad argument to 'format' (value out of range)"));
                }
            }
            // PUC `luaL_checkinteger` runs every integer-format operand through
            // `lua_tointegerx`, which itself accepts a numeric string and
            // converts via `lua_strx2number`. Without this coercion,
            // `string.format('%d', '13')` raises "attempt to format a string
            // value" — but PUC accepts it, and files.lua :611 feeds it the
            // string results of `os.date('%d')` etc.
            if let Value::Str(s) = arg
                && let Some(n) = crate::numeric::str2num(s.as_bytes(), true, true)
            {
                let arg = match n {
                    crate::numeric::Num::Int(i) => Value::Int(i),
                    crate::numeric::Num::Float(f) => Value::Float(f),
                };
                return vm.int_from(arg, "format");
            }
            if vm.version() <= crate::version::LuaVersion::Lua52
                && let Value::Float(f) = arg
                && f.is_finite()
            {
                // Go through u64 first so the PUC 5.2 "unsigned" probes
                // (`%u` / `%x` near 2^64) survive the cast. Then re-cast to
                // i64 with the same bit pattern so the downstream `fmt_int`
                // treats it as unsigned via `v as u64`. For `%d` / `%i` we
                // bound by the signed range (PUC 5.2 :211 expects `%d` to
                // error on `2^63`); for unsigned conversions we bound by u64.
                let t = f.trunc();
                // The float-to-integer conversion uses strict bounds because
                // `i64::MAX as f64` rounds *up* to 2^63 — without the strict
                // upper bound, `2^63` would slip past and the lossy cast
                // would silently saturate. strings.lua 5.2 :211 catches this.
                let i64max_f = i64::MAX as f64; // 2^63 exactly after rounding
                let u64max_f = u64::MAX as f64; // 2^64 exactly after rounding
                let bits = if signed {
                    if t >= (i64::MIN as f64) && t < i64max_f {
                        (t as i64) as u64
                    } else {
                        return vm.int_from(arg, "format");
                    }
                } else if t >= 0.0 && t < u64max_f {
                    // Unsigned conversions (%u/%x/%X/%o) reject negative
                    // operands in PUC 5.2: lstrlib.c's `tointeger` path uses
                    // an `unsigned int` cast that's UB on negative doubles,
                    // and the test bakes the error in (strings.lua :214).
                    t as u64
                } else {
                    return vm.int_from(arg, "format");
                };
                return Ok(bits as i64);
            }
            vm.int_from(arg, "format")
        };
        match conv {
            b'd' | b'i' => {
                let v = int_from_lenient(vm, arg, true)?;
                checkformat(vm, &form, b"-+0 ", true)?;
                let body = fmt_int(&spec, v, 10, false, true);
                pad(&mut out, body, &spec, PadKind::Int);
            }
            b'u' => {
                let v = int_from_lenient(vm, arg, false)?;
                checkformat(vm, &form, b"-0", true)?;
                let body = fmt_int(&spec, v, 10, false, false);
                pad(&mut out, body, &spec, PadKind::Int);
            }
            b'o' => {
                let v = int_from_lenient(vm, arg, false)?;
                checkformat(vm, &form, b"-#0", true)?;
                let body = fmt_int(&spec, v, 8, false, false);
                pad(&mut out, body, &spec, PadKind::Int);
            }
            b'x' | b'X' => {
                let v = int_from_lenient(vm, arg, false)?;
                checkformat(vm, &form, b"-#0", true)?;
                let body = fmt_int(&spec, v, 16, conv == b'X', false);
                pad(&mut out, body, &spec, PadKind::Int);
            }
            b'c' => {
                let v = int_from_lenient(vm, arg, true)?;
                checkformat(vm, &form, b"-", false)?;
                pad(&mut out, vec![v as u8], &spec, PadKind::Str);
            }
            b'e' | b'E' | b'f' | b'g' | b'G' | b'a' | b'A' => {
                let x = match arg {
                    Value::Int(n) => n as f64,
                    Value::Float(n) => n,
                    Value::Str(s) => crate::numeric::str2num(s.as_bytes(), true, true)
                        .map(|n| n.as_f64())
                        .ok_or_else(|| {
                            arg_error(vm, argi, "format", "number expected, got string")
                        })?,
                    v => {
                        return Err(arg_error(
                            vm,
                            argi,
                            "format",
                            &format!("number expected, got {}", v.type_name()),
                        ));
                    }
                };
                checkformat(vm, &form, b"-+#0 ", true)?;
                let prec = spec.prec.unwrap_or(6);
                let body = if x.is_nan() || x.is_infinite() {
                    let mut b = Vec::new();
                    if x.is_sign_negative() && !x.is_nan() {
                        b.push(b'-');
                    } else if spec.plus {
                        b.push(b'+');
                    }
                    b.extend_from_slice(if x.is_nan() { b"nan" } else { b"inf" });
                    if conv.is_ascii_uppercase() {
                        b.make_ascii_uppercase();
                    }
                    b
                } else {
                    let mut b = match conv.to_ascii_lowercase() {
                        b'f' => format!("{x:.prec$}").into_bytes(),
                        b'e' => fmt_exp(x, prec, conv == b'E'),
                        b'g' => fmt_g(x, prec, conv == b'G', spec.hash),
                        b'a' => fmt_hex_float(x, conv == b'A', spec.prec),
                        _ => unreachable!(),
                    };
                    // C `#` flag: force a radix point even when none would print
                    // (precision 0 for %f/%e drops it otherwise).
                    if spec.hash && !b.contains(&b'.') {
                        match b.iter().position(|&c| c == b'e' || c == b'E') {
                            Some(p) => b.insert(p, b'.'),
                            None => b.push(b'.'),
                        }
                    }
                    if x >= 0.0 && spec.plus {
                        b.insert(0, b'+');
                    } else if x >= 0.0 && spec.space {
                        b.insert(0, b' ');
                    }
                    b
                };
                pad(&mut out, body, &spec, PadKind::Float);
            }
            b'p' => {
                // 5.4+: object address (PUC lua_topointer)
                checkformat(vm, &form, b"-", false)?;
                let body = match arg {
                    Value::Str(s) => format!("{:p}", s.as_ptr()).into_bytes(),
                    Value::Table(t) => format!("{:p}", t.as_ptr()).into_bytes(),
                    Value::Closure(c) => format!("{:p}", c.as_ptr()).into_bytes(),
                    Value::Native(n) => format!("{:p}", n.as_ptr()).into_bytes(),
                    Value::Coro(c) => format!("{:p}", c.as_ptr()).into_bytes(),
                    Value::Userdata(u) => format!("{:p}", u.as_ptr()).into_bytes(),
                    _ => b"(null)".to_vec(),
                };
                pad(&mut out, body, &spec, PadKind::Str);
            }
            b's' => {
                let mut bytes = vm.tostring_value(arg)?;
                // PUC: with any modifier (form[2] != '\0') the string is passed
                // through C's sprintf, which can't carry embedded zeros, and the
                // spec is validated against the string flag set.
                if form.len() > 2 {
                    if bytes.contains(&0) {
                        return Err(arg_error(vm, argi, "format", "string contains zeros"));
                    }
                    checkformat(vm, &form, b"-", true)?;
                }
                if let Some(p) = spec.prec {
                    bytes.truncate(p);
                }
                pad(&mut out, bytes, &spec, PadKind::Str);
            }
            b'q' => {
                if spec.width != 0 || spec.prec.is_some() || spec.minus || spec.plus || spec.hash {
                    return Err(raise_str(vm, "specifier '%q' cannot have modifiers"));
                }
                quote_value(vm, arg, &mut out)?;
            }
            _ => {
                // PUC ≤5.3's unknown-conversion error says "invalid option";
                // 5.4 reworded to "invalid conversion". strings.lua 5.2/5.3
                // :302 / 5.4 :375 bake the matching substring per dialect.
                let head = if vm.version() <= crate::version::LuaVersion::Lua53 {
                    "invalid option"
                } else {
                    "invalid conversion"
                };
                return Err(raise_str(
                    vm,
                    &format!("{head} '{}' to 'format'", String::from_utf8_lossy(&form)),
                ));
            }
        }
    }
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}
