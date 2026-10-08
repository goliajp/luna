//! `utf8.codes`.

use super::*;

/// The `utf8.codes` iterator; upvalue [strict].
pub(crate) fn codes_iter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let strict = vm.nat_upval(fs, 0).truthy();
    let s = argcheck::check_string(vm, a, 0)?;
    let bytes = s.as_bytes();
    let len = bytes.len() as u64;
    // lua_tointeger: anything but an integral number reads as 0
    let control = match a.get(vm, 1) {
        Value::Int(i) => i,
        Value::Float(f) => crate::runtime::value::f2i_exact(f).unwrap_or(0),
        Value::Str(x) => match argcheck::to_num(vm, Value::Str(x)) {
            Some(crate::numeric::Num::Int(i)) => i,
            Some(crate::numeric::Num::Float(f)) => crate::runtime::value::f2i_exact(f).unwrap_or(0),
            None => 0,
        },
        _ => 0,
    };
    let n: u64 = if v == LuaVersion::Lua53 {
        // step past the previous character and its continuation bytes
        let n = control.wrapping_sub(1);
        if n < 0 {
            0
        } else if (n as u64) < len {
            let mut n = n as u64 + 1;
            while is_cont(at(bytes, n as usize)) {
                n += 1;
            }
            n
        } else {
            n as u64
        }
    } else {
        let mut n = control as u64;
        if n < len {
            while is_cont(at(bytes, n as usize)) {
                n += 1;
            }
        }
        n
    };
    if n >= len {
        return Ok(0);
    }
    match decode(v, bytes, n as usize, strict) {
        Some((code, next)) if !is_cont(at(bytes, next)) => {
            Ok(vm.nat_return(fs, &[Value::Int(n as i64 + 1), Value::Int(i64::from(code))]))
        }
        _ => Err(raise_str(vm, "invalid UTF-8 code")),
    }
}

pub(crate) fn u_codes(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let lax = v >= LuaVersion::Lua54 && a.get(vm, 1).truthy();
    let s = argcheck::check_string(vm, a, 0)?;
    if v >= LuaVersion::Lua54 && is_cont(at(s.as_bytes(), 0)) {
        return Err(arg_error(vm, 1, "invalid UTF-8 code"));
    }
    let it = vm.native_with(codes_iter, Box::new([Value::Bool(!lax)]));
    Ok(vm.nat_return(fs, &[it, Value::Str(s), Value::Int(0)]))
}
