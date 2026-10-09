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
        b's' if form.len() == 2 && plain_str(vm, a, arg, out) => {}
        b's' => {
            // a `__tostring` runs above the buffer's slot; its result stays
            // pushed over the errors that follow
            let buf = vm.buffer_slot(out.len());
            let s = vm.tostring_value_pushed(a.get(vm, arg), buf)?;
            if form.len() == 2 {
                out.extend_from_slice(&s);
            } else {
                if s.contains(&0) {
                    vm.native_push(1);
                    return Err(arg_error(vm, arg + 1, "string contains zeros"));
                }
                if let Err(e) = checkformat(vm, &form, b"-", true) {
                    vm.native_push(1);
                    return Err(e);
                }
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
