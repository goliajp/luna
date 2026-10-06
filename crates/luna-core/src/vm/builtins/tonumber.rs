//! `tonumber`: numerals in any base, and 5.1's `strtoul` fallback.

use super::*;

pub(crate) fn nat_tonumber(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() == LuaVersion::Lua51 {
        return tonumber_51(vm, a);
    }
    if a.is_none_or_nil(vm, 1) {
        let out = std_tonumber(vm, a)?;
        return Ok(vm.nat_return(fs, &[out]));
    }
    let out = if vm.version() == LuaVersion::Lua52 {
        // 5.2: the numeral may be given as a number; the base is a C int;
        // digits accumulate in a double.
        let s = argcheck::check_string(vm, a, 0)?;
        let base = argcheck::check_int(vm, a, 1)?;
        if !(2..=36).contains(&base) {
            return Err(arg_error(vm, 2, "base out of range"));
        }
        match str2number_base(s.as_bytes(), base as u32) {
            Some((neg, digits)) => {
                let n = digits.fold(0.0f64, |n, d| {
                    crate::numeric::c_mul_add(n, base as f64, d as f64)
                });
                Value::Float(if neg { -n } else { n })
            }
            None => Value::Nil,
        }
    } else {
        // 5.3+: the base is read first, the numeral must be a real string,
        // and digits accumulate in a wrapping unsigned integer.
        let base = argcheck::check_integer(vm, a, 1)?;
        let Value::Str(s) = a.get(vm, 0) else {
            return Err(argcheck::type_error(vm, a, 0, "string"));
        };
        if !(2..=36).contains(&base) {
            return Err(arg_error(vm, 2, "base out of range"));
        }
        match str2number_base(s.as_bytes(), base as u32) {
            Some((neg, digits)) => {
                let n = digits.fold(0u64, |n, d| {
                    n.wrapping_mul(base as u64).wrapping_add(d as u64)
                });
                Value::Int(if neg { n.wrapping_neg() } else { n } as i64)
            }
            None => Value::Nil,
        }
    };
    Ok(vm.nat_return(fs, &[out]))
}

/// `tonumber(v)` with no base on 5.2+: a number as is, a convertible string
/// converted, anything else nil — but an argument there must be.
fn std_tonumber(vm: &mut Vm, a: Args) -> Result<Value, LuaError> {
    let v = a.get(vm, 0);
    match v {
        Value::Int(_) | Value::Float(_) => return Ok(v),
        Value::Str(_) => {
            if let Some(n) = argcheck::to_num(vm, v) {
                return Ok(match n {
                    crate::numeric::Num::Int(i) => Value::Int(i),
                    crate::numeric::Num::Float(f) => Value::Float(f),
                });
            }
        }
        _ => {}
    }
    argcheck::check_any(vm, a, 0)?;
    Ok(Value::Nil)
}

/// C `isspace` / the `SPACECHARS` set of 5.2+'s `tonumber`.
fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// The digit value of an ASCII alphanumeric in bases up to 36.
fn alnum_digit(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 10),
        b'A'..=b'Z' => Some((c - b'A') as u32 + 10),
        _ => None,
    }
}

/// 5.2+'s based numeral: optional spaces, an optional sign, a run of
/// alphanumerics that must all be digits of `base`, optional spaces, and
/// nothing else. Returns the sign and the digits.
fn str2number_base(s: &[u8], base: u32) -> Option<(bool, impl Iterator<Item = u32> + '_)> {
    let start = s.iter().position(|&c| !is_c_space(c)).unwrap_or(s.len());
    let mut rest = &s[start..];
    let neg = rest.first() == Some(&b'-');
    if matches!(rest.first(), Some(b'-' | b'+')) {
        rest = &rest[1..];
    }
    let n = rest
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    if n == 0 || rest[n..].iter().any(|&c| !is_c_space(c)) {
        return None;
    }
    let digits = &rest[..n];
    if digits
        .iter()
        .any(|&c| alnum_digit(c).is_none_or(|d| d >= base))
    {
        return None;
    }
    Some((
        neg,
        digits
            .iter()
            .map(|&c| alnum_digit(c).expect("checked above")),
    ))
}

/// 5.1 `luaB_tonumber`. Base 10 — given or defaulted — is the ordinary
/// conversion; any other base goes through C `strtoul`, whose result becomes
/// a double.
#[cold]
fn tonumber_51(vm: &mut Vm, a: Args) -> Result<u32, LuaError> {
    let base = argcheck::opt_int(vm, a, 1, 10)?;
    if base == 10 {
        argcheck::check_any(vm, a, 0)?;
        let out = match a.get(vm, 0) {
            v @ (Value::Int(_) | Value::Float(_)) => v,
            v @ Value::Str(_) => match argcheck::to_num(vm, v) {
                Some(n) => Value::Float(n.as_f64()),
                None => Value::Nil,
            },
            _ => Value::Nil,
        };
        return Ok(vm.nat_return(a.fs, &[out]));
    }
    let s = argcheck::check_string(vm, a, 0)?;
    if !(2..=36).contains(&base) {
        return Err(arg_error(vm, 2, "base out of range"));
    }
    let out = match strtoul(s.as_bytes(), base as u32) {
        Some(n) => Value::Float(n as f64),
        None => Value::Nil,
    };
    Ok(vm.nat_return(a.fs, &[out]))
}

/// What 5.1's `tonumber(s, base)` accepts: C `strtoul` on the string as a C
/// string (it ends at the first NUL), then only trailing spaces. `strtoul`
/// takes a sign — negating the unsigned value — and, in base 16, a `0x`
/// prefix; an out-of-range value is clamped to `ULONG_MAX`, with ERANGE in
/// `errno`. `unsigned long` has 32 bits on Windows, 64 elsewhere.
fn strtoul(s: &[u8], base: u32) -> Option<u64> {
    const ULONG_MAX: u64 = if cfg!(windows) {
        u32::MAX as u64
    } else {
        u64::MAX
    };
    let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
    let mut i = s.iter().position(|&c| !is_c_space(c)).unwrap_or(s.len());
    let neg = s.get(i) == Some(&b'-');
    if matches!(s.get(i), Some(b'-' | b'+')) {
        i += 1;
    }
    if base == 16
        && s.get(i) == Some(&b'0')
        && matches!(s.get(i + 1), Some(b'x' | b'X'))
        && s.get(i + 2)
            .and_then(|&c| alnum_digit(c))
            .is_some_and(|d| d < 16)
    {
        i += 2;
    }
    let digits_start = i;
    let mut n: u64 = 0;
    let mut overflow = false;
    while let Some(d) = s.get(i).and_then(|&c| alnum_digit(c)).filter(|&d| d < base) {
        match n
            .checked_mul(base as u64)
            .and_then(|n| n.checked_add(d as u64))
            .filter(|&n| n <= ULONG_MAX)
        {
            Some(v) => n = v,
            None => overflow = true,
        }
        i += 1;
    }
    if overflow {
        crate::cerrno::set(crate::cerrno::ERANGE);
    }
    if i == digits_start || s[i..].iter().any(|&c| !is_c_space(c)) {
        return None;
    }
    Some(if overflow {
        ULONG_MAX
    } else if neg {
        n.wrapping_neg() & ULONG_MAX
    } else {
        n
    })
}
