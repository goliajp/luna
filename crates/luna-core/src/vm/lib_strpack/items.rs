//! The items of `string.pack` and `string.unpack`, read from the format
//! one at a time.

use super::*;

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
    while fp < fmt.len() {
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
                let len = unpack_int(vm, &data[p..], h.islittle, size as usize, false)? as u64;
                if len > ld - *pos - size {
                    // 5.3's `*pos + len + size <= ld` wraps for a huge length
                    // and lets it through to the string allocation
                    if v == LuaVersion::Lua53 && (*pos).wrapping_add(len).wrapping_add(size) <= ld {
                        return Err(vm.plain_err("memory allocation error: block too big"));
                    }
                    return Err(arg_error(vm, 2, "data string too short"));
                }
                let st = p + size as usize;
                let s = vm.alloc_str(&data[st..st + len as usize])?;
                results.push(Value::Str(s))?;
                *pos += len;
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
            let expected = match opt {
                KOption::Char | KOption::Str | KOption::Zstr => "string",
                _ => "number",
            };
            return Err(arg_error(
                vm,
                *arg,
                &format!("{expected} expected, got nil"),
            ));
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
            KOption::Str => {
                let s = argcheck::check_string(vm, a, i)?;
                let len = s.len() as u64;
                if !(size >= SZINT || len < (1u64 << (size * 8))) {
                    return Err(arg_error(
                        vm,
                        *arg,
                        "string length does not fit in given size",
                    ));
                }
                reserve(vm, out, size + len)?;
                pack_int(vm, out, len, h.islittle, size as usize, false)?;
                vm.buf_push(out, s.as_bytes())?;
                *totalsize += len;
            }
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
