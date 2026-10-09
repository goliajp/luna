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
mod item54;
mod quoted;
use item54::item54;
pub(crate) use quoted::*;

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
    // room for an item or two past the format before the buffer regrows
    let mut out = Vec::with_capacity(fmt.len() + 16);
    let mut arg = 0u32;
    let mut i = 0;
    // the buffer's slot counts only when a callback runs or an error is
    // raised: it is added then, from the content built so far
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
            let buf = vm.buffer_slot(out.len());
            vm.native_push(buf);
            return Err(arg_error(vm, arg + 1, "no value"));
        }
        let item = if v >= LuaVersion::Lua54 {
            item54(vm, a, arg, fmt, i, &mut out)
        } else {
            item51(vm, a, arg, fmt, i, &mut out)
        };
        i = match item {
            Ok(i) => i,
            Err(e) => {
                let buf = vm.buffer_slot(out.len());
                vm.native_push(buf);
                return Err(e);
            }
        };
    }
    let s = vm.built_str(&out)?;
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
        b's' if v >= LuaVersion::Lua53 && body.is_empty() && plain_str(vm, a, arg, out) => {}
        b's' => {
            let s = if v == LuaVersion::Lua51 {
                argcheck::check_string(vm, a, arg)?.as_bytes().to_vec()
            } else {
                // a `__tostring` runs above the buffer's slot
                let buf = vm.buffer_slot(out.len());
                if v == LuaVersion::Lua52 {
                    tolstring_52(vm, a.get(vm, arg), buf)?
                } else {
                    vm.tostring_value_pushed(a.get(vm, arg), buf)?
                }
            };
            let has_prec = body.contains(&b'.');
            if v >= LuaVersion::Lua53 && body.is_empty() {
                out.extend_from_slice(&s);
            } else {
                if v >= LuaVersion::Lua53 && s.contains(&0) {
                    // over the `luaL_tolstring` result
                    vm.native_push(1);
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
/// 5.2's `luaL_tolstring` does not check what `__tostring` returns: a
/// value that is not a string or a number formats as C's `%s` of a null
/// pointer.
fn tolstring_52(vm: &mut Vm, v: Value, extra: u32) -> Result<Vec<u8>, LuaError> {
    let mm = vm.get_mm(v, crate::vm::exec::Mm::ToString);
    if mm.is_nil() {
        return vm.tostring_value(v);
    }
    match vm
        .call_value_pushed(mm, &[v], extra)?
        .first()
        .copied()
        .unwrap_or(Value::Nil)
    {
        Value::Str(s) => Ok(s.as_bytes().to_vec()),
        r @ (Value::Int(_) | Value::Float(_)) => Ok(vm.tostring_basic(r)),
        _ => Ok(b"(null)".to_vec()),
    }
}

fn c_int_cast(x: f64) -> i32 {
    x as i32
}

/// Append argument `arg` when it is a string `luaL_tolstring` gives back
/// as it is (no `__tostring` in the way); false, appending nothing, when it
/// is not.
fn plain_str(vm: &mut Vm, a: Args, arg: u32, out: &mut Vec<u8>) -> bool {
    let v = a.get(vm, arg);
    match v {
        Value::Str(s) if vm.get_mm(v, crate::vm::exec::Mm::ToString).is_nil() => {
            out.extend_from_slice(s.as_bytes());
            true
        }
        _ => false,
    }
}
