//! `%q` and `%p`: quoted literals and pointers.

use super::*;

/// `lua_topointer`: collectable objects by address, a light userdata's own
/// pointer, everything else NULL.
/// The addresses match what `tostring` prints.
pub(crate) fn topointer(v: Value) -> Option<usize> {
    match v {
        Value::Str(s) => Some(s.as_ptr() as usize),
        Value::Table(t) => Some(t.as_ptr() as usize),
        Value::Closure(c) => Some(c.as_ptr() as usize),
        Value::Native(n) => Some(n.as_ptr() as usize),
        Value::Coro(c) => Some(c.as_ptr() as usize),
        Value::Userdata(u) => Some(u.as_ptr() as usize),
        Value::LightUserdata(p) => (!p.is_null()).then_some(p as usize),
        Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_) => None,
    }
}

/// `addquoted`: a string as a Lua literal. 5.1 escapes only the bytes that
/// would break the literal; later versions write every control byte in
/// decimal, padded to three digits when a digit follows.
pub(crate) fn addquoted(v: LuaVersion, s: &[u8], out: &mut Vec<u8>) {
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
pub(crate) fn addliteral(
    vm: &mut Vm,
    a: Args,
    arg: u32,
    out: &mut Vec<u8>,
) -> Result<(), LuaError> {
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
pub(crate) fn quotefloat(v: LuaVersion, x: f64, out: &mut Vec<u8>) {
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
