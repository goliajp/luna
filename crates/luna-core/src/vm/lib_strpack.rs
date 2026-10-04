//! string.pack / string.unpack / string.packsize — a port of PUC's lstrlib
//! pack engine as it stands in each of 5.3, 5.4 and 5.5. Binary
//! (de)serialization of integers, floats, and strings with explicit size /
//! endianness / alignment control.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::lib_string::MAX_STR;

mod format;
#[cfg(test)]
mod tests;

use format::*;

/// Maximum size for the binary representation of an integer.
const MAXINTSIZE: u64 = 16;
/// `sizeof(lua_Integer)`.
const SZINT: u64 = 8;
/// Native endianness assumed by the `=` option (our targets are little).
const NATIVE_LITTLE: bool = true;
/// Native max alignment (`offsetof(struct cD, u)`) on the reference
/// platform.
const NATIVE_MAXALIGN: u64 = 8;

/// Pack `n` into `size` bytes, sign-extending past eight bytes when `neg`.
fn pack_int(out: &mut Vec<u8>, n: u64, islittle: bool, size: usize, neg: bool) {
    let at = out.len();
    for i in 0..size {
        let b = if i < SZINT as usize {
            (n >> (8 * i)) as u8
        } else if neg {
            0xff
        } else {
            0
        };
        out.push(b);
    }
    if !islittle {
        out[at..].reverse();
    }
}

/// Unpack a `size`-byte integer from `bytes[..size]`, sign-extending or
/// checking the bytes past eight as PUC does.
fn unpack_int(
    vm: &mut Vm,
    bytes: &[u8],
    islittle: bool,
    size: usize,
    issigned: bool,
) -> Result<i64, LuaError> {
    let at = |i: usize| bytes[if islittle { i } else { size - 1 - i }];
    let limit = size.min(SZINT as usize);
    let mut res: u64 = 0;
    for i in (0..limit).rev() {
        res = (res << 8) | u64::from(at(i));
    }
    if size < SZINT as usize {
        if issigned {
            let mask = 1u64 << (size * 8 - 1);
            res = (res ^ mask).wrapping_sub(mask);
        }
    } else if size > SZINT as usize {
        let fill = if !issigned || (res as i64) >= 0 {
            0
        } else {
            0xff
        };
        if (limit..size).any(|i| at(i) != fill) {
            return Err(raise_str(
                vm,
                &format!("{size}-byte integer does not fit into Lua Integer"),
            ));
        }
    }
    Ok(res as i64)
}

fn float_bytes(out: &mut Vec<u8>, mut b: Vec<u8>, islittle: bool) {
    if !islittle {
        b.reverse();
    }
    out.extend_from_slice(&b);
}

/// Grow the result by `n` bytes, or fail as the allocator would.
fn reserve(vm: &mut Vm, out: &[u8], n: u64) -> Result<(), LuaError> {
    if n > MAX_STR - (out.len() as u64).min(MAX_STR) {
        return Err(vm.mem_err());
    }
    Ok(())
}

/// `luaL_checkstring`: the format is read as a C string.
fn format_arg(vm: &mut Vm, a: Args) -> Result<Vec<u8>, LuaError> {
    let f = argcheck::check_string(vm, a, 0)?;
    let b = f.as_bytes();
    Ok(b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())].to_vec())
}

pub(crate) fn s_pack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let fmt = format_arg(vm, a)?;
    let v55 = vm.version() >= LuaVersion::Lua55;
    let mut h = Header::new();
    let mut out: Vec<u8> = Vec::new();
    let mut totalsize: u64 = 0;
    // 1-based index of the last argument consumed
    let mut arg: u32 = 1;
    let mut fp = 0usize;
    while fp < fmt.len() {
        let (opt, size, ntoalign) = getdetails(vm, &mut h, totalsize, &fmt, &mut fp)?;
        if v55 && size + ntoalign > max_size(vm) - totalsize {
            return Err(arg_error(vm, arg, "result too long"));
        }
        totalsize += ntoalign + size;
        out.resize(out.len() + ntoalign as usize, 0);
        arg += 1;
        let i = arg - 1;
        // str_pack pushes a nil mark above its arguments, so the first
        // missing one reads as nil rather than "no value"
        if a.is_none(i) && !matches!(opt, KOption::Padding | KOption::PadAlign | KOption::Nop) {
            let expected = match opt {
                KOption::Char | KOption::Str | KOption::Zstr => "string",
                _ => "number",
            };
            return Err(arg_error(vm, arg, &format!("{expected} expected, got nil")));
        }
        match opt {
            KOption::Int => {
                let n = argcheck::check_integer(vm, a, i)?;
                if size < SZINT {
                    let lim = 1i64 << (size * 8 - 1);
                    if !(-lim <= n && n < lim) {
                        return Err(arg_error(vm, arg, "integer overflow"));
                    }
                }
                pack_int(&mut out, n as u64, h.islittle, size as usize, n < 0);
            }
            KOption::Uint => {
                let n = argcheck::check_integer(vm, a, i)?;
                if size < SZINT && (n as u64) >= (1u64 << (size * 8)) {
                    return Err(arg_error(vm, arg, "unsigned overflow"));
                }
                pack_int(&mut out, n as u64, h.islittle, size as usize, false);
            }
            KOption::Float => {
                let x = argcheck::check_number(vm, a, i)? as f32;
                float_bytes(&mut out, x.to_le_bytes().to_vec(), h.islittle);
            }
            KOption::Number => {
                let x = argcheck::check_number(vm, a, i)?;
                float_bytes(&mut out, x.to_le_bytes().to_vec(), h.islittle);
            }
            KOption::Char => {
                let s = argcheck::check_string(vm, a, i)?;
                let len = s.len() as u64;
                if len > size {
                    return Err(arg_error(vm, arg, "string longer than given size"));
                }
                reserve(vm, &out, size)?;
                out.extend_from_slice(s.as_bytes());
                out.resize(out.len() + (size - len) as usize, 0);
            }
            KOption::Str => {
                let s = argcheck::check_string(vm, a, i)?;
                let len = s.len() as u64;
                if !(size >= SZINT || len < (1u64 << (size * 8))) {
                    return Err(arg_error(
                        vm,
                        arg,
                        "string length does not fit in given size",
                    ));
                }
                reserve(vm, &out, size + len)?;
                pack_int(&mut out, len, h.islittle, size as usize, false);
                out.extend_from_slice(s.as_bytes());
                totalsize += len;
            }
            KOption::Zstr => {
                let s = argcheck::check_string(vm, a, i)?;
                if s.as_bytes().contains(&0) {
                    return Err(arg_error(vm, arg, "string contains zeros"));
                }
                reserve(vm, &out, s.len() as u64 + 1)?;
                out.extend_from_slice(s.as_bytes());
                out.push(0);
                totalsize += s.len() as u64 + 1;
            }
            KOption::Padding => {
                out.push(0);
                arg -= 1;
            }
            KOption::PadAlign | KOption::Nop => arg -= 1,
        }
    }
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

pub(crate) fn s_packsize(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let fmt = format_arg(vm, a)?;
    let v53 = vm.version() == LuaVersion::Lua53;
    let mut h = Header::new();
    let mut totalsize: u64 = 0;
    let mut fp = 0usize;
    while fp < fmt.len() {
        let (opt, size, ntoalign) = getdetails(vm, &mut h, totalsize, &fmt, &mut fp)?;
        let variable = opt == KOption::Str || opt == KOption::Zstr;
        // 5.3 checks the size before the kind
        if variable && !v53 {
            return Err(arg_error(vm, 1, "variable-length format"));
        }
        let size = size + ntoalign;
        if size > max_size(vm) || totalsize > max_size(vm) - size {
            return Err(arg_error(vm, 1, "format result too large"));
        }
        totalsize += size;
        if variable {
            return Err(arg_error(vm, 1, "variable-length format"));
        }
    }
    Ok(vm.nat_return(fs, &[Value::Int(totalsize as i64)]))
}

pub(crate) fn s_unpack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let fmt = format_arg(vm, a)?;
    let d = argcheck::check_string(vm, a, 1)?;
    let data = d.as_bytes();
    let ld = data.len() as u64;
    let init = argcheck::opt_integer(vm, a, 2, 1)?;
    // 5.3 `posrelat` leaves 0 and far-negative positions at 0, which the
    // bounds check rejects; 5.4's `posrelatI` clips them to 1
    let rel = if init >= 0 {
        init
    } else if init.unsigned_abs() > ld {
        0
    } else {
        ld as i64 + init + 1
    };
    let rel = if v >= LuaVersion::Lua54 {
        rel.max(1)
    } else {
        rel
    };
    if rel == 0 || rel as u64 - 1 > ld {
        return Err(arg_error(vm, 3, "initial position out of string"));
    }
    let mut pos = rel as u64 - 1;
    let mut h = Header::new();
    let mut results: Vec<Value> = Vec::new();
    let mut fp = 0usize;
    while fp < fmt.len() {
        let (opt, size, ntoalign) = getdetails(vm, &mut h, pos, &fmt, &mut fp)?;
        if pos > ld || ntoalign + size > ld - pos {
            return Err(arg_error(vm, 2, "data string too short"));
        }
        pos += ntoalign;
        argcheck::check_stack(vm, a, results.len() as i64 + 2, "too many results")?;
        let p = pos as usize;
        match opt {
            KOption::Int | KOption::Uint => {
                let n = unpack_int(
                    vm,
                    &data[p..],
                    h.islittle,
                    size as usize,
                    opt == KOption::Int,
                )?;
                results.push(Value::Int(n));
            }
            KOption::Float => {
                let mut b: [u8; 4] = data[p..p + 4].try_into().expect("4 bytes");
                if !h.islittle {
                    b.reverse();
                }
                results.push(Value::Float(f64::from(f32::from_le_bytes(b))));
            }
            KOption::Number => {
                let mut b: [u8; 8] = data[p..p + 8].try_into().expect("8 bytes");
                if !h.islittle {
                    b.reverse();
                }
                results.push(Value::Float(f64::from_le_bytes(b)));
            }
            KOption::Char => {
                let s = vm.heap.intern(&data[p..p + size as usize]);
                results.push(Value::Str(s));
            }
            KOption::Str => {
                let len = unpack_int(vm, &data[p..], h.islittle, size as usize, false)? as u64;
                if len > ld - pos - size {
                    // 5.3's `pos + len + size <= ld` wraps for a huge length
                    // and lets it through to the string allocation
                    if v == LuaVersion::Lua53 && pos.wrapping_add(len).wrapping_add(size) <= ld {
                        return Err(vm.plain_err("memory allocation error: block too big"));
                    }
                    return Err(arg_error(vm, 2, "data string too short"));
                }
                let st = p + size as usize;
                let s = vm.heap.intern(&data[st..st + len as usize]);
                results.push(Value::Str(s));
                pos += len;
            }
            KOption::Zstr => {
                // strlen stops at the string's terminating zero at worst;
                // 5.3 accepts that, later versions call it unfinished
                let len = data[p..].iter().position(|&b| b == 0);
                if len.is_none() && v >= LuaVersion::Lua54 {
                    return Err(arg_error(vm, 2, "unfinished string for format 'z'"));
                }
                let len = len.unwrap_or(data.len() - p);
                let s = vm.heap.intern(&data[p..p + len]);
                results.push(Value::Str(s));
                pos += len as u64 + 1;
            }
            KOption::Padding | KOption::PadAlign | KOption::Nop => {}
        }
        pos += size;
    }
    results.push(Value::Int((pos + 1) as i64));
    Ok(vm.nat_return(fs, &results))
}
