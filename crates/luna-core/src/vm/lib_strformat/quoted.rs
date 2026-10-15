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
pub(crate) fn addquoted(vm: &mut Vm, s: &[u8], out: &mut LVec<u8>) -> Result<(), LuaError> {
    let v = vm.version();
    vm.buf_reserve(out, s.len() + 2)?;
    vm.buf_push(out, b"\"")?;
    for (i, &c) in s.iter().enumerate() {
        match c {
            b'"' | b'\\' | b'\n' => vm.buf_push(out, &[b'\\', c])?,
            b'\r' if v == LuaVersion::Lua51 => vm.buf_push(out, b"\\r")?,
            0 if v == LuaVersion::Lua51 => vm.buf_push(out, b"\\000")?,
            c if v >= LuaVersion::Lua52 && c.is_ascii_control() => {
                // a decimal escape, three digits when a digit follows
                let wide = s.get(i + 1).is_some_and(u8::is_ascii_digit);
                let d = [b'\\', b'0' + c / 100, b'0' + c / 10 % 10, b'0' + c % 10];
                let from = if wide || c >= 100 {
                    1
                } else if c >= 10 {
                    2
                } else {
                    3
                };
                vm.buf_push(out, &[d[0]])?;
                vm.buf_push(out, &d[from..])?;
            }
            c => vm.buf_push(out, &[c])?,
        }
    }
    vm.buf_push(out, b"\"")
}

/// 5.3+ `addliteral`: any value that has a literal form.
pub(crate) fn addliteral(
    vm: &mut Vm,
    a: Args,
    arg: u32,
    out: &mut LVec<u8>,
) -> Result<Added, LuaError> {
    let mut item = Vec::new();
    match a.get(vm, arg) {
        Value::Str(s) => {
            addquoted(vm, s.as_bytes(), out)?;
            return Ok(Added::Chars);
        }
        Value::Int(n) if n == i64::MIN => {
            // "-9223372036854775808" would read back as a float
            item.extend_from_slice(b"0x8000000000000000");
        }
        Value::Int(n) => cfmt::signed(out, &Spec::default(), n),
        Value::Float(x) => quotefloat(vm.version(), x, &mut item),
        v @ (Value::Nil | Value::Bool(_)) => {
            // `luaL_tolstring` and `luaL_addvalue`
            let s = vm.tolstring(v, 0)?;
            vm.buf_push(out, s.as_bytes())?;
            return Ok(Added::Value(s));
        }
        _ => return Err(arg_error(vm, arg + 1, "value has no literal form")),
    }
    vm.buf_push(out, &item)?;
    Ok(Added::Sized)
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
