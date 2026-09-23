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
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
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
    unsafe { t.as_mut() }
        .set(&mut vm.heap, k, gmatch_v)
        .expect("valid key");
    if vm.version() == crate::version::LuaVersion::Lua51 {
        let k = Value::Str(vm.heap.intern(b"gfind"));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, gmatch_v)
            .expect("valid key");
    }
    set(vm, "gsub", s_gsub);
    set(vm, "format", crate::vm::lib_strformat::s_format);
    set(vm, "dump", s_dump);
    // string.pack/unpack/packsize landed in 5.3 — 5.1/5.2 should not see them
    if vm.version() >= crate::version::LuaVersion::Lua53 {
        set(vm, "pack", crate::vm::lib_strpack::s_pack);
        set(vm, "unpack", crate::vm::lib_strpack::s_unpack);
        set(vm, "packsize", crate::vm::lib_strpack::s_packsize);
    }
    vm.set_global("string", Value::Table(t))
        .expect("stdlib registration");
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
        Value::Float(f) => crate::runtime::value::f2i_exact(f)
            .ok_or_else(|| arg_error(vm, i + 1, "number has no integer representation")),
        Value::Str(s) => match crate::numeric::str2num(s.as_bytes(), true, true) {
            Some(Num::Int(x)) => Ok(x),
            Some(Num::Float(f)) => crate::runtime::value::f2i_exact(f)
                .ok_or_else(|| arg_error(vm, i + 1, "number has no integer representation")),
            None => Err(arg_error(vm, i + 1, "number expected, got string")),
        },
        v => {
            let tn = vm.obj_typename(v);
            Err(arg_error(vm, i + 1, &format!("number expected, got {tn}")))
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

/// PUC `luaL_checkinteger` semantics for a numeric argument: ints
/// pass, integral floats convert, numeric STRINGS convert (the libc
/// `lua_tointegerx` string leg), and anything else raises the
/// standard `bad argument #N to 'who' (number expected, got T)` —
/// not a bespoke wording (v2.14 CV.2, fixture 5.5/330).
pub(crate) fn check_int_arg(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    i: u32,
    who: &str,
) -> Result<i64, LuaError> {
    let v = vm.nat_arg(fs, nargs, i);
    let num = match v {
        Value::Int(n) => Some(crate::numeric::Num::Int(n)),
        Value::Float(f) => Some(crate::numeric::Num::Float(f)),
        Value::Str(s) => crate::numeric::str2num(s.as_bytes(), true, true),
        _ => None,
    };
    match num {
        Some(crate::numeric::Num::Int(n)) => Ok(n),
        Some(crate::numeric::Num::Float(f)) => match crate::runtime::value::f2i_exact(f) {
            Some(n) => Ok(n),
            None => Err(vm.rt_err("number has no integer representation")),
        },
        None => Err(arg_error(
            vm,
            i + 1,
            &format!("number expected, got {}", v.type_name()),
        )),
    }
}

const MAX_STR: usize = 1 << 30;

fn s_rep(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "rep")?;
    let n = check_int_arg(vm, fs, nargs, 1, "rep")?;
    let sep: Vec<u8> = match vm.nat_arg(fs, nargs, 2) {
        Value::Nil => Vec::new(),
        Value::Str(x) => x.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                3,
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let piece = s.len() + sep.len();
    // `piece == 0` (both the string and the separator are empty) must
    // short-circuit alongside `n <= 0`: the result is the empty string for
    // any `n`, but the loop below would otherwise spin `n` times copying
    // zero bytes — `string.rep("", math.maxinteger, "")` hangs the VM.
    // The size check does not catch it, since `0 * n` never exceeds
    // MAX_STR. Matches PUC 5.5.1's `if (n <= 0 || (len | lsep) == 0)`
    // (lstrlib.c:144); PUC 5.5.0 and earlier hang here exactly as we did.
    if n <= 0 || piece == 0 {
        let v = Value::Str(vm.heap.intern(b""));
        return Ok(vm.nat_return(fs, &[v]));
    }
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
            return Err(arg_error(vm, i + 1, "value out of range"));
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
                &format!("string/function/table expected, got {}", v.type_name()),
            ));
        }
    }
    let max_n = match vm.nat_arg(fs, nargs, 3) {
        Value::Nil => i64::MAX,
        _ => check_int_arg(vm, fs, nargs, 3, "gsub")?,
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
