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
        unsafe { t.as_mut() }.set(k, fv).expect("valid key");
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
    set(vm, "gmatch", s_gmatch);
    set(vm, "gsub", s_gsub);
    set(vm, "format", s_format);
    vm.set_global("string", Value::Table(t));
    // shared string metatable: methods resolve through the library table
    let mt = vm.heap.new_table();
    let idx = Value::Str(vm.heap.intern(b"__index"));
    unsafe { mt.as_mut() }
        .set(idx, Value::Table(t))
        .expect("valid key");
    vm.set_string_metatable(Some(mt));
}

fn check_str(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<Gc<LuaStr>, LuaError> {
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

fn opt_int(vm: &mut Vm, fs: u32, nargs: u32, i: u32, default: i64) -> Result<i64, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Nil => Ok(default),
        v => vm.int_from(v, "use as an index"),
    }
}

/// PUC posrelat: translate 1-based/negative positions.
fn posrelat(pos: i64, len: usize) -> i64 {
    if pos >= 0 {
        pos
    } else if (-pos) as usize > len {
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

fn s_sub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "sub")?;
    let len = s.len();
    let mut i = posrelat(opt_int(vm, fs, nargs, 1, 1)?, len);
    let mut j = posrelat(opt_int(vm, fs, nargs, 2, -1)?, len);
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
    let i = posrelat(opt_int(vm, fs, nargs, 1, 1)?, len).max(1);
    let j = posrelat(opt_int(vm, fs, nargs, 2, i)?, len).min(len as i64);
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
    len: usize,
) -> Result<Option<usize>, LuaError> {
    let raw = posrelat(opt_int(vm, fs, nargs, arg, 1)?, len);
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
    let Some(init) = init_offset(vm, fs, nargs, 2, src.len())? else {
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
    let Some(init) = init_offset(vm, fs, nargs, 2, src.len())? else {
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

/// gmatch iterator: upvalues [src, pat, pos].
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
    let src = s.as_bytes().to_vec();
    let pat = p.as_bytes().to_vec();
    let pos = pos as usize;
    if pos > src.len() {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    match pattern::find(&src, &pat, pos).map_err(|e| pat_err(vm, e))? {
        Some(m) => {
            // empty matches advance by one to guarantee progress
            let next = if m.end > m.start { m.end } else { m.start + 1 };
            vm.nat_set_upval(fs, 2, Value::Int(next as i64));
            let mut out = Vec::new();
            push_captures(vm, &src, &m, &mut out);
            Ok(vm.nat_return(fs, &out))
        }
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

fn s_gmatch(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_str(vm, fs, nargs, 0, "gmatch")?;
    let p = check_str(vm, fs, nargs, 1, "gmatch")?;
    let it = vm.native_with(
        gmatch_iter,
        Box::new([Value::Str(s), Value::Str(p), Value::Int(0)]),
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
    let mut out: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut count: i64 = 0;
    while count < max_n {
        let Some(m) = pattern::find(&src, &pat, pos).map_err(|e| pat_err(vm, e))? else {
            break;
        };
        out.extend_from_slice(&src[pos..m.start]); // unmatched span
        count += 1;
        gsub_one(vm, &src, &m, repl, &mut out)?;
        if m.end > m.start {
            pos = m.end;
        } else {
            // empty match: copy one byte and advance to guarantee progress
            if m.start < src.len() {
                out.push(src[m.start]);
            }
            pos = m.start + 1;
        }
        if pos > src.len() {
            break;
        }
    }
    if pos <= src.len() {
        out.extend_from_slice(&src[pos..]);
    }
    let res = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[res, Value::Int(count)]))
}

/// One replacement (PUC add_value): string template, table lookup, or call.
fn gsub_one(
    vm: &mut Vm,
    src: &[u8],
    m: &pattern::Match,
    repl: Value,
    out: &mut Vec<u8>,
) -> Result<(), LuaError> {
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
            return Ok(());
        }
        Value::Int(_) | Value::Float(_) => {
            let bytes = vm.tostring_basic(repl);
            out.extend_from_slice(&bytes);
            return Ok(());
        }
        Value::Table(t) => {
            let k = cap_value(vm, 0)?;
            t.get(k)
        }
        f @ (Value::Closure(_) | Value::Native(_)) => {
            let mut args = Vec::new();
            push_captures(vm, src, m, &mut args);
            vm.call_value(f, &args)?
                .first()
                .copied()
                .unwrap_or(Value::Nil)
        }
        _ => unreachable!(),
    };
    match result {
        Value::Nil | Value::Bool(false) => out.extend_from_slice(whole),
        v => append_value(vm, v, out)?,
    }
    Ok(())
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

fn pad(out: &mut Vec<u8>, body: Vec<u8>, spec: &Spec, numeric: bool) {
    let w = spec.width;
    if body.len() >= w {
        out.extend_from_slice(&body);
        return;
    }
    let fill = w - body.len();
    if spec.minus {
        out.extend_from_slice(&body);
        out.extend(std::iter::repeat_n(b' ', fill));
    } else if spec.zero && numeric && spec.prec.is_none() {
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
fn fmt_hex_float(v: f64, upper: bool) -> Vec<u8> {
    let bits = v.to_bits();
    let sign = if bits >> 63 != 0 { "-" } else { "" };
    let exp_bits = ((bits >> 52) & 0x7FF) as i64;
    let frac = bits & ((1u64 << 52) - 1);
    let s = if v.is_nan() {
        format!("{sign}nan")
    } else if v.is_infinite() {
        format!("{sign}inf")
    } else if exp_bits == 0 && frac == 0 {
        format!("{sign}0x0p+0")
    } else {
        let (lead, exp, mant) = if exp_bits == 0 {
            (0u64, -1022i64, frac)
        } else {
            (1u64, exp_bits - 1023, frac)
        };
        let mut hex = format!("{mant:013x}");
        while hex.len() > 1 && hex.ends_with('0') {
            hex.pop();
        }
        if mant == 0 {
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
                    b'"' => out.extend_from_slice(b"\\\""),
                    b'\\' => out.extend_from_slice(b"\\\\"),
                    b'\n' => out.extend_from_slice(b"\\n"),
                    b'\r' => out.extend_from_slice(b"\\r"),
                    0 => {
                        // \0, or \000 when a digit follows
                        if bytes.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
                            out.extend_from_slice(b"\\000");
                        } else {
                            out.extend_from_slice(b"\\0");
                        }
                    }
                    c if c.is_ascii_control() => {
                        if bytes.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
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
        Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Value::Float(f) => {
            if f.is_nan() {
                out.extend_from_slice(b"(0/0)");
            } else if f.is_infinite() {
                out.extend_from_slice(if f < 0.0 { b"-1e9999" } else { b"1e9999" });
            } else if f == f.floor() {
                // integral float: keep readability and the float subtype
                out.extend_from_slice(format!("{f:.1}").as_bytes());
            } else {
                out.extend_from_slice(&fmt_hex_float(f, false));
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
        // flags
        let mut spec = Spec {
            minus: false,
            plus: false,
            space: false,
            hash: false,
            zero: false,
            width: 0,
            prec: None,
        };
        while let Some(&c) = fmt.get(i) {
            match c {
                b'-' => spec.minus = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.hash = true,
                b'0' => spec.zero = true,
                _ => break,
            }
            i += 1;
        }
        let mut wd = 0usize;
        let mut wn = 0;
        while let Some(&c @ b'0'..=b'9') = fmt.get(i) {
            wd = wd * 10 + (c - b'0') as usize;
            wn += 1;
            i += 1;
        }
        if wn > 2 {
            return Err(raise_str(vm, "invalid format string to 'format'"));
        }
        spec.width = wd;
        if fmt.get(i) == Some(&b'.') {
            i += 1;
            let mut p = 0usize;
            let mut pn = 0;
            while let Some(&c @ b'0'..=b'9') = fmt.get(i) {
                p = p * 10 + (c - b'0') as usize;
                pn += 1;
                i += 1;
            }
            if pn > 2 {
                return Err(raise_str(vm, "invalid format string to 'format'"));
            }
            spec.prec = Some(p);
        }
        let Some(&conv) = fmt.get(i) else {
            return Err(raise_str(vm, "invalid conversion to 'format'"));
        };
        i += 1;
        if argi >= nargs && conv != b'%' {
            return Err(arg_error(vm, argi + 1, "format", "no value"));
        }
        let arg = vm.nat_arg(fs, nargs, argi);
        argi += 1;
        match conv {
            b'd' | b'i' => {
                let v = vm.int_from(arg, "format")?;
                let body = fmt_int(&spec, v, 10, false, true);
                pad(&mut out, body, &spec, true);
            }
            b'u' => {
                let v = vm.int_from(arg, "format")?;
                let body = fmt_int(&spec, v, 10, false, false);
                pad(&mut out, body, &spec, true);
            }
            b'o' => {
                let v = vm.int_from(arg, "format")?;
                let body = fmt_int(&spec, v, 8, false, false);
                pad(&mut out, body, &spec, true);
            }
            b'x' | b'X' => {
                let v = vm.int_from(arg, "format")?;
                let body = fmt_int(&spec, v, 16, conv == b'X', false);
                pad(&mut out, body, &spec, true);
            }
            b'c' => {
                let v = vm.int_from(arg, "format")?;
                pad(&mut out, vec![v as u8], &spec, false);
            }
            b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A' => {
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
                        b'a' => fmt_hex_float(x, conv == b'A'),
                        _ => unreachable!(),
                    };
                    if x >= 0.0 && spec.plus {
                        b.insert(0, b'+');
                    } else if x >= 0.0 && spec.space {
                        b.insert(0, b' ');
                    }
                    b
                };
                pad(&mut out, body, &spec, true);
            }
            b's' => {
                let mut bytes = vm.tostring_value(arg)?;
                if let Some(p) = spec.prec {
                    bytes.truncate(p);
                }
                pad(&mut out, bytes, &spec, false);
            }
            b'q' => {
                if spec.width != 0 || spec.prec.is_some() || spec.minus || spec.plus || spec.hash {
                    return Err(raise_str(vm, "specifier '%q' cannot have modifiers"));
                }
                quote_value(vm, arg, &mut out)?;
            }
            c => {
                return Err(raise_str(
                    vm,
                    &format!("invalid conversion '%{}' to 'format'", c as char),
                ));
            }
        }
    }
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}
