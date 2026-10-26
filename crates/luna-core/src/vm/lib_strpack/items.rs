//! The items of `string.pack` and `string.unpack`, read from the format
//! one at a time.

use super::*;
use crate::runtime::puc::fmt::FmtArg;

/// The items of `string.unpack` from `pos`, read into `results`.
pub(super) fn unpack_items(
    vm: &mut Vm,
    a: Args,
    fmt: &[u8],
    data: &[u8],
    pos: &mut u64,
    results: &mut LVec<Value>,
) -> Result<(), LuaError> {
    let v = vm.version();
    let ld = data.len() as u64;
    let mut h = Header::new();
    let mut fp = 0usize;
    let pushed = vm.running_natives.last().map_or(0, |a| a.top_off);
    while fp < fmt.len() {
        // the results read so far are pushed: an item that raises, or the
        // check before it, finds them under the top
        vm.native_set_pushed(pushed + results.len() as i32);
        let (opt, size, ntoalign) = getdetails(vm, &mut h, *pos, fmt, &mut fp)?;
        if *pos > ld || ntoalign + size > ld - *pos {
            return Err(arg_error(vm, 2, "data string too short"));
        }
        *pos += ntoalign;
        // two slots are checked for before each item, and each result is
        // pushed once it is read
        let top = Args::new(a.fs, a.n + results.len() as u32);
        argcheck::check_stack(vm, top, 2, "too many results")?;
        let p = *pos as usize;
        match opt {
            KOption::Int | KOption::Uint => {
                let n = unpack_int(
                    vm,
                    &data[p..],
                    h.islittle,
                    size as usize,
                    opt == KOption::Int,
                )?;
                results.push(Value::Int(n))?;
            }
            KOption::Float => {
                let mut b: [u8; 4] = data[p..p + 4].try_into().expect("4 bytes");
                if !h.islittle {
                    b.reverse();
                }
                results.push(Value::Float(f64::from(f32::from_le_bytes(b))))?;
            }
            KOption::Number => {
                let mut b: [u8; 8] = data[p..p + 8].try_into().expect("8 bytes");
                if !h.islittle {
                    b.reverse();
                }
                results.push(Value::Float(f64::from_le_bytes(b)))?;
            }
            KOption::Char => {
                let s = vm.alloc_str(&data[p..p + size as usize])?;
                results.push(Value::Str(s))?;
            }
            KOption::Str => {
                let s = unpack_str(vm, data, *pos, size, h.islittle)?;
                *pos += s.len() as u64;
                results.push(Value::Str(s))?;
            }
            KOption::Zstr => {
                // strlen stops at the string's terminating zero at worst;
                // 5.3 accepts that, later versions call it unfinished
                let len = data[p..].iter().position(|&b| b == 0);
                if len.is_none() && v >= LuaVersion::Lua54 {
                    return Err(arg_error(vm, 2, "unfinished string for format 'z'"));
                }
                let len = len.unwrap_or(data.len() - p);
                let s = vm.alloc_str(&data[p..p + len])?;
                results.push(Value::Str(s))?;
                *pos += len as u64 + 1;
            }
            KOption::Padding | KOption::PadAlign | KOption::Nop => {}
        }
        *pos += size;
    }
    Ok(())
}

/// The error for the missing argument `arg` an option `opt` wants: the
/// nil mark makes it read as nil (`luaL_typeerror` on it: its `__name`
/// looked up, then the message pushed before `luaL_argerror` adds to it).
fn missing_arg(vm: &mut Vm, opt: KOption, arg: u32) -> LuaError {
    let expected = match opt {
        KOption::Char | KOption::Str | KOption::Zstr => "string",
        _ => "number",
    };
    if let Err(e) = vm.metafield_check(Value::Nil, b"__name") {
        return e;
    }
    let fmt = b"%s expected, got %s";
    let args = [FmtArg::S(expected.as_bytes()), FmtArg::S(b"nil")];
    match argcheck::pushed_extra(vm, 2, fmt, &args) {
        Ok(m) => arg_error(vm, arg, &m),
        Err(e) => e,
    }
}

/// Option `s`: the string argument `i` after its length in `size` bytes;
/// how many bytes the string added.
fn pack_str(
    vm: &mut Vm,
    a: Args,
    i: u32,
    size: u64,
    little: bool,
    out: &mut LVec<u8>,
) -> Result<u64, LuaError> {
    let s = argcheck::check_string(vm, a, i)?;
    let len = s.len() as u64;
    // `sizeof(size_t)` before 5.5, `sizeof(lua_Unsigned)` after
    let whole = if vm.version() >= LuaVersion::Lua55 {
        SZINT
    } else {
        crate::vm::c_abi::SIZEOF_SIZE_T
    };
    if !(size >= whole || len < (1u64 << (size * 8))) {
        return Err(arg_error(
            vm,
            i + 1,
            "string length does not fit in given size",
        ));
    }
    reserve(vm, out, size + len)?;
    pack_int(vm, out, len, little, size as usize, false)?;
    vm.buf_push(out, s.as_bytes())?;
    Ok(len)
}

/// The items of `string.pack`, packed into `out`.
#[allow(clippy::too_many_arguments)]
pub(super) fn pack_items(
    vm: &mut Vm,
    a: Args,
    fmt: &[u8],
    v55: bool,
    h: &mut Header,
    out: &mut LVec<u8>,
    totalsize: &mut u64,
    arg: &mut u32,
    fp: &mut usize,
) -> Result<(), LuaError> {
    while *fp < fmt.len() {
        let (opt, size, ntoalign) = getdetails(vm, h, *totalsize, fmt, fp)?;
        if v55 && size + ntoalign > max_size(vm) - *totalsize {
            return Err(arg_error(vm, *arg, "result too long"));
        }
        *totalsize += ntoalign + size;
        zeros(vm, out, ntoalign as usize)?;
        *arg += 1;
        let i = *arg - 1;
        // the nil mark makes the first missing argument read as nil
        if a.is_none(i) && !matches!(opt, KOption::Padding | KOption::PadAlign | KOption::Nop) {
            return Err(missing_arg(vm, opt, *arg));
        }
        match opt {
            KOption::Int => {
                let n = argcheck::check_integer(vm, a, i)?;
                if size < SZINT {
                    let lim = 1i64 << (size * 8 - 1);
                    if !(-lim <= n && n < lim) {
                        return Err(arg_error(vm, *arg, "integer overflow"));
                    }
                }
                pack_int(vm, out, n as u64, h.islittle, size as usize, n < 0)?;
            }
            KOption::Uint => {
                let n = argcheck::check_integer(vm, a, i)?;
                if size < SZINT && (n as u64) >= (1u64 << (size * 8)) {
                    return Err(arg_error(vm, *arg, "unsigned overflow"));
                }
                pack_int(vm, out, n as u64, h.islittle, size as usize, false)?;
            }
            KOption::Float => {
                let x = argcheck::check_number(vm, a, i)? as f32;
                float_bytes(vm, out, &mut x.to_le_bytes(), h.islittle)?;
            }
            KOption::Number => {
                let x = argcheck::check_number(vm, a, i)?;
                float_bytes(vm, out, &mut x.to_le_bytes(), h.islittle)?;
            }
            KOption::Char => {
                let s = argcheck::check_string(vm, a, i)?;
                let len = s.len() as u64;
                if len > size {
                    return Err(arg_error(vm, *arg, "string longer than given size"));
                }
                reserve(vm, out, size)?;
                vm.buf_push(out, s.as_bytes())?;
                zeros(vm, out, (size - len) as usize)?;
            }
            KOption::Str => *totalsize += pack_str(vm, a, i, size, h.islittle, out)?,
            KOption::Zstr => {
                let s = argcheck::check_string(vm, a, i)?;
                if s.as_bytes().contains(&0) {
                    return Err(arg_error(vm, *arg, "string contains zeros"));
                }
                reserve(vm, out, s.len() as u64 + 1)?;
                vm.buf_push(out, s.as_bytes())?;
                vm.buf_push(out, &[0])?;
                *totalsize += s.len() as u64 + 1;
            }
            KOption::Padding => {
                vm.buf_push(out, &[0])?;
                *arg -= 1;
            }
            KOption::PadAlign | KOption::Nop => *arg -= 1,
        }
    }
    Ok(())
}

/// Option `s` at `pos`: a length in `size` bytes, then that many bytes of
/// string.
fn unpack_str(
    vm: &mut Vm,
    data: &[u8],
    pos: u64,
    size: u64,
    little: bool,
) -> Result<crate::runtime::Gc<crate::runtime::LuaStr>, LuaError> {
    let p = pos as usize;
    let ld = data.len() as u64;
    let len = unpack_int(vm, &data[p..], little, size as usize, false)? as u64;
    // 5.3 and 5.4 read it as a `size_t`
    let len = if vm.version() <= LuaVersion::Lua54 {
        len as usize as u64
    } else {
        len
    };
    if len > ld - pos - size {
        // 5.3's `*pos + len + size <= ld` wraps (in `size_t`) for
        // a huge length and lets it through to the allocation
        let end = (pos as usize)
            .wrapping_add(len as usize)
            .wrapping_add(size as usize);
        if vm.version() == LuaVersion::Lua53 && end as u64 <= ld {
            return Err(vm.runerror_in_c("memory allocation error: block too big"));
        }
        return Err(arg_error(vm, 2, "data string too short"));
    }
    let st = p + size as usize;
    Ok(vm.alloc_str(&data[st..st + len as usize])?)
}
