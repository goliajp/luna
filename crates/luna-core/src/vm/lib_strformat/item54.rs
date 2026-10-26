//! One `string.format` conversion under 5.4 and 5.5.

use super::*;

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
#[inline]
pub(super) fn item54(
    vm: &mut Vm,
    a: Args,
    arg: u32,
    fmt: &[u8],
    start: usize,
    out: &mut LVec<u8>,
) -> Result<(usize, Added), LuaError> {
    let mut added = Added::Sized;
    // every conversion but a long `%s` and `%q` fits in this
    vm.buf_reserve(out, cfmt::ITEM_MAX)?;
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
    // the length check above keeps it within `MAX_FORMAT`
    let mut fbuf = [0u8; MAX_FORMAT];
    let n = p - start;
    fbuf[0] = b'%';
    fbuf[1..=n].copy_from_slice(&fmt[start..p]);
    fbuf[n + 1] = conv;
    let form = &fbuf[..n + 2];
    let sp = Spec::parse(&fmt[start..p]);
    match conv {
        b'c' => {
            checkformat(vm, form, b"-", false)?;
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
            checkformat(vm, form, flags, true)?;
            if let b'd' | b'i' = conv {
                cfmt::signed(out, &sp, n);
            } else {
                cfmt::unsigned(out, &sp, conv, n as u64);
            }
        }
        b'a' | b'A' => {
            checkformat(vm, form, b"-+#0 ", true)?;
            let x = argcheck::check_number(vm, a, arg)?;
            cfmt::float(out, &sp, conv, x);
        }
        b'f' | b'e' | b'E' | b'g' | b'G' => {
            let x = argcheck::check_number(vm, a, arg)?;
            checkformat(vm, form, b"-+#0 ", true)?;
            cfmt::float(out, &sp, conv, x);
        }
        b'p' => {
            let ptr = topointer(a.get(vm, arg));
            checkformat(vm, form, b"-", false)?;
            match ptr {
                Some(ptr) => cfmt::pointer(out, &sp, ptr),
                None => cfmt::cstr(out, &sp, b"(null)"),
            }
        }
        b'q' => {
            if form.len() > 2 {
                return Err(raise_str(vm, "specifier '%q' cannot have modifiers"));
            }
            added = addliteral(vm, a, arg, out)?;
        }
        b's' => {
            if form.len() == 2
                && let Some(s) = plain_str(vm, a, arg)?
            {
                vm.buf_push(out, s.as_bytes())?;
                return Ok((p + 1, Added::Value(s)));
            }
            added = string_item(vm, a, arg, out, form, &sp)?;
        }
        _ => return Err(invalid_conversion(vm, form)),
    }
    Ok((p + 1, added))
}

/// `%s` of a value not taken as it is: `luaL_tolstring` (whose
/// `__tostring` runs above the buffer's slot, its result left pushed over
/// the errors that follow), then the bytes formatted by `form`.
#[cfg_attr(luna_hot, inline(always))]
fn string_item(
    vm: &mut Vm,
    a: Args,
    arg: u32,
    out: &mut LVec<u8>,
    form: &[u8],
    sp: &Spec,
) -> Result<Added, LuaError> {
    // a `__tostring` runs above the buffer's slot; its result stays
    // pushed over the errors that follow
    let buf = vm.buffer_slot(out.len());
    vm.native_hold_top(&[Vm::buffer_box()][..buf as usize])?;
    let v = a.get(vm, arg);
    let gs = vm.tolstring(v, buf)?;
    let s = gs.as_bytes().to_vec();
    if form.len() == 2 {
        vm.buf_push(out, &s)?;
        return Ok(Added::Value(gs));
    }
    if s.contains(&0) {
        vm.native_push(1);
        return Err(arg_error(vm, arg + 1, "string contains zeros"));
    }
    if let Err(e) = checkformat(vm, form, b"-", true) {
        vm.native_push(1);
        return Err(e);
    }
    if !form.contains(&b'.') && s.len() >= 100 {
        vm.buf_push(out, &s)?;
        return Ok(Added::Value(gs));
    }
    cfmt::cstr(out, sp, &s);
    Ok(Added::Sized)
}

/// The error for the conversion `form`, printed as a C string.
fn invalid_conversion(vm: &mut Vm, form: &[u8]) -> LuaError {
    let shown = &form[..form.iter().position(|&b| b == 0).unwrap_or(form.len())];
    let mut msg = b"invalid conversion '".to_vec();
    msg.extend_from_slice(shown);
    msg.extend_from_slice(b"' to 'format'");
    raise_bytes(vm, &msg)
}
