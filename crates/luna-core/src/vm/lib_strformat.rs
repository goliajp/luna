//! `string.format`, one driver per generation of PUC's `str_format`:
//! 5.1/5.2 pass numbers through C casts and truncate at zero bytes, 5.3
//! reads integers exactly, 5.4 validates every specification's flags.
//! The items themselves are rendered by [`crate::vm::cfmt`].

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_bytes, raise_str};
use crate::vm::cfmt::{self, Spec};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// PUC `MAX_FORMAT`: a 5.4+ specification plus '%', a length modifier
/// and the terminator must fit in 32 bytes.
const MAX_FORMAT: usize = 32;
const FLAGS: &[u8] = b"-+ #0";
/// 2^63, the first double past `i64`.
const TWO63: f64 = 9_223_372_036_854_775_808.0;

pub(crate) fn s_format(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let f = argcheck::check_string(vm, a, 0)?;
    let fmt = f.as_bytes();
    let v = vm.version();
    let mut out = Vec::with_capacity(fmt.len());
    let mut arg = 0u32;
    let mut i = 0;
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        // a '%' at the very end pairs with the string's terminating zero
        if fmt.get(i) == Some(&b'%') {
            out.push(b'%');
            i += 1;
            continue;
        }
        arg += 1;
        if arg >= nargs {
            return Err(arg_error(vm, arg + 1, "no value"));
        }
        i = if v >= LuaVersion::Lua54 {
            item54(vm, a, arg, fmt, i, &mut out)?
        } else {
            item51(vm, a, arg, fmt, i, &mut out)?
        };
    }
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

fn byte_at(fmt: &[u8], i: usize) -> u8 {
    fmt.get(i).copied().unwrap_or(0)
}

/// ≤5.3 `scanformat`: at most five flag bytes, two width digits, two
/// precision digits; the next byte is the conversion, whatever it is.
/// Returns the position of the conversion byte.
fn scanformat(vm: &mut Vm, fmt: &[u8], start: usize) -> Result<usize, LuaError> {
    let mut p = start;
    while fmt.get(p).is_some_and(|c| FLAGS.contains(c)) {
        p += 1;
    }
    if p - start > FLAGS.len() {
        return Err(raise_str(vm, "invalid format (repeated flags)"));
    }
    let digit = |p: usize| fmt.get(p).is_some_and(u8::is_ascii_digit);
    for _ in 0..2 {
        if digit(p) {
            p += 1;
        }
    }
    if fmt.get(p) == Some(&b'.') {
        p += 1;
        for _ in 0..2 {
            if digit(p) {
                p += 1;
            }
        }
    }
    if digit(p) {
        return Err(raise_str(
            vm,
            "invalid format (width or precision too long)",
        ));
    }
    Ok(p)
}

/// One conversion under 5.1, 5.2 or 5.3. Returns where scanning resumes.
fn item51(
    vm: &mut Vm,
    a: Args,
    arg: u32,
    fmt: &[u8],
    start: usize,
    out: &mut Vec<u8>,
) -> Result<usize, LuaError> {
    let v = vm.version();
    let p = scanformat(vm, fmt, start)?;
    let conv = byte_at(fmt, p);
    let body = &fmt[start..p.min(fmt.len())];
    let sp = Spec::parse(body);
    match conv {
        b'c' => {
            let c = match v {
                LuaVersion::Lua51 => c_int_cast(argcheck::check_number(vm, a, arg)?),
                LuaVersion::Lua52 => argcheck::check_int(vm, a, arg)?,
                _ => argcheck::check_integer(vm, a, arg)? as i32,
            };
            let mark = out.len();
            cfmt::char(out, &sp, c as u8);
            // 5.1 copies the item with strlen, so a zero byte ends it
            if v == LuaVersion::Lua51
                && let Some(z) = out[mark..].iter().position(|&b| b == 0)
            {
                out.truncate(mark + z);
            }
        }
        b'd' | b'i' => {
            let n = match v {
                LuaVersion::Lua51 => argcheck::check_number(vm, a, arg)? as i64,
                // PUC casts and checks that the cast lost less than one; a
                // value outside the integer range must fail, which is what
                // the check does where the cast yields the "indefinite"
                // value (the reference test suite asserts it for 2^63)
                LuaVersion::Lua52 => {
                    let x = argcheck::check_number(vm, a, arg)?;
                    if !(-TWO63..TWO63).contains(&x) {
                        return Err(arg_error(vm, arg + 1, "not a number in proper range"));
                    }
                    x as i64
                }
                _ => argcheck::check_integer(vm, a, arg)?,
            };
            cfmt::signed(out, &sp, n);
        }
        b'o' | b'u' | b'x' | b'X' => {
            let n = match v {
                LuaVersion::Lua51 => argcheck::check_number(vm, a, arg)? as u64,
                LuaVersion::Lua52 => {
                    let x = argcheck::check_number(vm, a, arg)?;
                    if !(x > -1.0 && x < 2.0 * TWO63) {
                        return Err(arg_error(
                            vm,
                            arg + 1,
                            "not a non-negative number in proper range",
                        ));
                    }
                    x as u64
                }
                _ => argcheck::check_integer(vm, a, arg)? as u64,
            };
            cfmt::unsigned(out, &sp, conv, n);
        }
        b'a' | b'A' if v >= LuaVersion::Lua52 => {
            let x = argcheck::check_number(vm, a, arg)?;
            cfmt::float(out, &sp, conv, x);
        }
        b'e' | b'E' | b'f' | b'g' | b'G' => {
            let x = argcheck::check_number(vm, a, arg)?;
            cfmt::float(out, &sp, conv, x);
        }
        b'q' => match v {
            LuaVersion::Lua51 | LuaVersion::Lua52 => {
                let s = argcheck::check_string(vm, a, arg)?;
                addquoted(v, s.as_bytes(), out);
            }
            _ => addliteral(vm, a, arg, out)?,
        },
        b's' => {
            let s = if v == LuaVersion::Lua51 {
                argcheck::check_string(vm, a, arg)?.as_bytes().to_vec()
            } else {
                vm.tostring_value(a.get(vm, arg))?
            };
            let has_prec = body.contains(&b'.');
            if v >= LuaVersion::Lua53 && body.is_empty() {
                out.extend_from_slice(&s);
            } else {
                if v >= LuaVersion::Lua53 && s.contains(&0) {
                    return Err(arg_error(vm, arg + 1, "string contains zeros"));
                }
                if !has_prec && s.len() >= 100 {
                    out.extend_from_slice(&s);
                } else {
                    cfmt::cstr(out, &sp, &s);
                }
            }
        }
        _ => {
            // how each version's `lua_pushfstring` renders the "%c"
            let mut msg = b"invalid option '%".to_vec();
            match v {
                LuaVersion::Lua51 if conv == 0 => {}
                LuaVersion::Lua53 if !(0x20..0x7F).contains(&conv) => {
                    msg.extend_from_slice(format!("<\\{conv}>").as_bytes());
                }
                _ => msg.push(conv),
            }
            msg.extend_from_slice(b"' to 'format'");
            return Err(raise_bytes(vm, &msg));
        }
    }
    Ok(p + 1)
}

/// C's `(int)` conversion of a double on the reference platform: a
/// saturating one, NaN to zero.
fn c_int_cast(x: f64) -> i32 {
    x as i32
}

/// 5.4+ `checkformat`: only `flags`, then (unless the width starts with
/// '0') two width digits and, where allowed, '.' and two precision digits.
fn checkformat(vm: &mut Vm, form: &[u8], flags: &[u8], precision: bool) -> Result<(), LuaError> {
    let mut k = 1;
    while form.get(k).is_some_and(|c| flags.contains(c)) {
        k += 1;
    }
    let digit = |k: usize| form.get(k).is_some_and(u8::is_ascii_digit);
    if form.get(k) != Some(&b'0') {
        for _ in 0..2 {
            if digit(k) {
                k += 1;
            }
        }
        if form.get(k) == Some(&b'.') && precision {
            k += 1;
            for _ in 0..2 {
                if digit(k) {
                    k += 1;
                }
            }
        }
    }
    if !form.get(k).is_some_and(u8::is_ascii_alphabetic) {
        let mut msg = b"invalid conversion specification: '".to_vec();
        msg.extend_from_slice(form);
        msg.push(b'\'');
        return Err(raise_bytes(vm, &msg));
    }
    Ok(())
}

/// One conversion under 5.4 or 5.5. Returns where scanning resumes.
fn item54(
    vm: &mut Vm,
    a: Args,
    arg: u32,
    fmt: &[u8],
    start: usize,
    out: &mut Vec<u8>,
) -> Result<usize, LuaError> {
    // getformat: flags, width and precision bytes ('0' counted as a flag),
    // then the conversion
    let mut p = start;
    while fmt.get(p).is_some_and(|c| b"-+#0 123456789.".contains(c)) {
        p += 1;
    }
    if p - start + 1 >= MAX_FORMAT - 10 {
        return Err(raise_str(vm, "invalid format (too long)"));
    }
    let conv = byte_at(fmt, p);
    let mut form = Vec::with_capacity(p - start + 2);
    form.push(b'%');
    form.extend_from_slice(&fmt[start..p]);
    form.push(conv);
    let sp = Spec::parse(&fmt[start..p]);
    match conv {
        b'c' => {
            checkformat(vm, &form, b"-", false)?;
            let c = argcheck::check_integer(vm, a, arg)? as i32;
            cfmt::char(out, &sp, c as u8);
        }
        b'd' | b'i' | b'u' | b'o' | b'x' | b'X' => {
            let n = argcheck::check_integer(vm, a, arg)?;
            let flags: &[u8] = match conv {
                b'd' | b'i' => b"-+0 ",
                b'u' => b"-0",
                _ => b"-#0",
            };
            checkformat(vm, &form, flags, true)?;
            if let b'd' | b'i' = conv {
                cfmt::signed(out, &sp, n);
            } else {
                cfmt::unsigned(out, &sp, conv, n as u64);
            }
        }
        b'a' | b'A' => {
            checkformat(vm, &form, b"-+#0 ", true)?;
            let x = argcheck::check_number(vm, a, arg)?;
            cfmt::float(out, &sp, conv, x);
        }
        b'f' | b'e' | b'E' | b'g' | b'G' => {
            let x = argcheck::check_number(vm, a, arg)?;
            checkformat(vm, &form, b"-+#0 ", true)?;
            cfmt::float(out, &sp, conv, x);
        }
        b'p' => {
            let ptr = topointer(a.get(vm, arg));
            checkformat(vm, &form, b"-", false)?;
            match ptr {
                Some(ptr) => cfmt::pointer(out, &sp, ptr),
                None => cfmt::cstr(out, &sp, b"(null)"),
            }
        }
        b'q' => {
            if form.len() > 2 {
                return Err(raise_str(vm, "specifier '%q' cannot have modifiers"));
            }
            addliteral(vm, a, arg, out)?;
        }
        b's' => {
            let s = vm.tostring_value(a.get(vm, arg))?;
            if form.len() == 2 {
                out.extend_from_slice(&s);
            } else {
                if s.contains(&0) {
                    return Err(arg_error(vm, arg + 1, "string contains zeros"));
                }
                checkformat(vm, &form, b"-", true)?;
                if !form.contains(&b'.') && s.len() >= 100 {
                    out.extend_from_slice(&s);
                } else {
                    cfmt::cstr(out, &sp, &s);
                }
            }
        }
        _ => {
            // the message prints 'form' as a C string
            let shown = &form[..form.iter().position(|&b| b == 0).unwrap_or(form.len())];
            let mut msg = b"invalid conversion '".to_vec();
            msg.extend_from_slice(shown);
            msg.extend_from_slice(b"' to 'format'");
            return Err(raise_bytes(vm, &msg));
        }
    }
    Ok(p + 1)
}

/// `lua_topointer`: collectable objects by address, everything else NULL.
/// The addresses match what `tostring` prints.
fn topointer(v: Value) -> Option<usize> {
    match v {
        Value::Str(s) => Some(s.as_ptr() as usize),
        Value::Table(t) => Some(t.as_ptr() as usize),
        Value::Closure(c) => Some(c.as_ptr() as usize),
        Value::Native(n) => Some(n.as_ptr() as usize),
        Value::Coro(c) => Some(c.as_ptr() as usize),
        Value::Userdata(u) => Some(u.as_ptr() as usize),
        Value::LightUserdata(p) => Some(p as usize),
        Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_) => None,
    }
}

/// `addquoted`: a string as a Lua literal. 5.1 escapes only the bytes that
/// would break the literal; later versions write every control byte in
/// decimal, padded to three digits when a digit follows.
fn addquoted(v: LuaVersion, s: &[u8], out: &mut Vec<u8>) {
    out.push(b'"');
    for (i, &c) in s.iter().enumerate() {
        match c {
            b'"' | b'\\' | b'\n' => {
                out.push(b'\\');
                out.push(c);
            }
            b'\r' if v == LuaVersion::Lua51 => out.extend_from_slice(b"\\r"),
            0 if v == LuaVersion::Lua51 => out.extend_from_slice(b"\\000"),
            c if v >= LuaVersion::Lua52 && c.is_ascii_control() => {
                if s.get(i + 1).is_some_and(u8::is_ascii_digit) {
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

/// 5.3+ `addliteral`: any value that has a literal form.
fn addliteral(vm: &mut Vm, a: Args, arg: u32, out: &mut Vec<u8>) -> Result<(), LuaError> {
    match a.get(vm, arg) {
        Value::Str(s) => addquoted(vm.version(), s.as_bytes(), out),
        Value::Int(n) if n == i64::MIN => {
            // "-9223372036854775808" would read back as a float
            out.extend_from_slice(format!("0x{:x}", n as u64).as_bytes());
        }
        Value::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::Float(x) => quotefloat(vm.version(), x, out),
        v @ (Value::Nil | Value::Bool(_)) => {
            let s = vm.tostring_value(v)?;
            out.extend_from_slice(&s);
        }
        _ => return Err(arg_error(vm, arg + 1, "value has no literal form")),
    }
    Ok(())
}

/// A float as a hexadecimal numeral; 5.4 spells the values `%a` cannot
/// read back as numerals that can.
fn quotefloat(v: LuaVersion, x: f64, out: &mut Vec<u8>) {
    if v >= LuaVersion::Lua54 {
        if x == f64::INFINITY {
            return out.extend_from_slice(b"1e9999");
        } else if x == f64::NEG_INFINITY {
            return out.extend_from_slice(b"-1e9999");
        } else if x.is_nan() {
            return out.extend_from_slice(b"(0/0)");
        }
    }
    cfmt::float(out, &Spec::default(), b'a', x);
}
