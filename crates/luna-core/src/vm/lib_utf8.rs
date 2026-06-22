//! utf8 library. Lua's UTF-8 is byte-oriented and "extended" (accepts code
//! points up to 2^31-1 in lax mode; strict mode enforces real UTF-8 limits).

use crate::runtime::Value;
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_utf8(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }.set(&mut vm.heap, k, fv).expect("valid key");
    };
    set(vm, "char", u_char);
    set(vm, "codepoint", u_codepoint);
    set(vm, "len", u_len);
    set(vm, "offset", u_offset);
    set(vm, "codes", u_codes);
    let k = Value::Str(vm.heap.intern(b"charpattern"));
    let v = Value::Str(vm.heap.intern(b"[\x00-\x7F\xC2-\xFD][\x80-\xBF]*"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }.set(&mut vm.heap, k, v).expect("valid key");
    vm.set_global("utf8", Value::Table(t)).expect("stdlib registration");
    vm.barrier_back_table(t);
}

const MAX_UNICODE: u32 = 0x10_FFFF;

/// Decode one sequence at `i`; returns (code point, next index) or None.
fn decode(s: &[u8], i: usize, strict: bool) -> Option<(u32, usize)> {
    let c = *s.get(i)?;
    if c < 0x80 {
        return Some((c as u32, i + 1));
    }
    if c < 0xC0 {
        return None; // continuation byte cannot start a sequence
    }
    let count = match c {
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        0xF8..=0xFB => 4,
        0xFC..=0xFD => 5,
        _ => return None,
    };
    let mut cp = (c as u32) & (0x7F >> count);
    for k in 1..=count {
        let cc = *s.get(i + k)?;
        if cc & 0xC0 != 0x80 {
            return None;
        }
        cp = (cp << 6) | (cc as u32 & 0x3F);
    }
    // overlong/oversized checks (PUC limits per length)
    const LIMITS: [u32; 6] = [0x80, 0x800, 0x10000, 0x200000, 0x4000000, 0x8000_0000];
    if count >= 6 || cp < LIMITS[count - 1] {
        return None;
    }
    if strict && (cp > MAX_UNICODE || (0xD800..=0xDFFF).contains(&cp)) {
        return None;
    }
    Some((cp, i + 1 + count))
}

fn encode(out: &mut Vec<u8>, x: u32) {
    if x < 0x80 {
        out.push(x as u8);
        return;
    }
    let mut cont = [0u8; 6];
    let mut n = 0;
    let mut mfb: u32 = 0x3f;
    let mut x = x;
    loop {
        cont[n] = 0x80 | (x & 0x3f) as u8;
        n += 1;
        x >>= 6;
        mfb >>= 1;
        if x <= mfb {
            break;
        }
    }
    out.push(((!mfb << 1) | x) as u8);
    out.extend(cont[..n].iter().rev());
}

fn check_strv(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<Vec<u8>, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Str(s) => Ok(s.as_bytes().to_vec()),
        v => Err(arg_error(
            vm,
            i + 1,
            who,
            &format!("string expected, got {}", v.type_name()),
        )),
    }
}

fn posrelat(pos: i64, len: usize) -> i64 {
    if pos >= 0 {
        pos
    } else if (-pos) as usize > len {
        0
    } else {
        len as i64 + pos + 1
    }
}

fn u_char(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let mut out = Vec::new();
    // PUC 5.3 capped utf8.char at 0x10FFFF (Unicode max); 5.4 extended to
    // 0x7FFFFFFF (the "extended UTF-8" 31-bit range). utf8.lua bakes the
    // tighter limit for 5.3.
    let cap: i64 = if vm.version() <= crate::version::LuaVersion::Lua53 {
        0x10FFFF
    } else {
        0x7FFF_FFFF
    };
    for i in 0..nargs {
        let c = vm.int_from(vm.nat_arg(fs, nargs, i), "use as a code point")?;
        if !(0..=cap).contains(&c) {
            return Err(arg_error(vm, i + 1, "char", "value out of range"));
        }
        encode(&mut out, c as u32);
    }
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

fn u_codepoint(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_strv(vm, fs, nargs, 0, "codepoint")?;
    let i = if nargs >= 2 {
        posrelat(
            vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?,
            s.len(),
        )
    } else {
        1
    };
    let j = if nargs >= 3 && !vm.nat_arg(fs, nargs, 2).is_nil() {
        posrelat(
            vm.int_from(vm.nat_arg(fs, nargs, 2), "use as an index")?,
            s.len(),
        )
    } else {
        i
    };
    let lax = vm.nat_arg(fs, nargs, 3).truthy();
    // Same wording flip as `offset` above (5.3 "out of range" → 5.4
    // "out of bounds"). utf8.lua codepoint tests bake the substring.
    let oob = if vm.version() <= crate::version::LuaVersion::Lua53 {
        "out of range"
    } else {
        "out of bounds"
    };
    if i < 1 {
        return Err(arg_error(vm, 2, "codepoint", oob));
    }
    if j > s.len() as i64 {
        return Err(arg_error(vm, 3, "codepoint", oob));
    }
    let mut out = Vec::new();
    let mut pos = (i - 1) as usize;
    while pos < j as usize {
        let Some((cp, next)) = decode(&s, pos, !lax) else {
            return Err(raise_str(vm, "invalid UTF-8 code"));
        };
        out.push(Value::Int(cp as i64));
        pos = next;
    }
    Ok(vm.nat_return(fs, &out))
}

fn u_len(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_strv(vm, fs, nargs, 0, "len")?;
    let i = if nargs >= 2 && !vm.nat_arg(fs, nargs, 1).is_nil() {
        posrelat(
            vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?,
            s.len(),
        )
    } else {
        1
    };
    let j = if nargs >= 3 && !vm.nat_arg(fs, nargs, 2).is_nil() {
        posrelat(
            vm.int_from(vm.nat_arg(fs, nargs, 2), "use as an index")?,
            s.len(),
        )
    } else {
        -1_i64 + s.len() as i64 + 1
    };
    let lax = vm.nat_arg(fs, nargs, 3).truthy();
    // PUC utflen bounds: --posi must land in [0, len]; --posj must be < len
    if !(i >= 1 && i - 1 <= s.len() as i64) {
        return Err(arg_error(vm, 2, "len", "initial position out of bounds"));
    }
    if j > s.len() as i64 {
        return Err(arg_error(vm, 3, "len", "final position out of bounds"));
    }
    let mut n: i64 = 0;
    let mut pos = (i - 1) as usize;
    let end = j as usize;
    while pos < end {
        match decode(&s, pos, !lax) {
            Some((_, next)) => {
                n += 1;
                pos = next;
            }
            None => {
                // fail: nil + position of the invalid byte (1-based)
                return Ok(vm.nat_return(fs, &[Value::Nil, Value::Int(pos as i64 + 1)]));
            }
        }
    }
    Ok(vm.nat_return(fs, &[Value::Int(n)]))
}

fn is_cont(b: u8) -> bool {
    b & 0xC0 == 0x80
}

fn u_offset(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = check_strv(vm, fs, nargs, 0, "offset")?;
    let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    let len = s.len();
    let default_i = if n >= 0 { 1 } else { len as i64 + 1 };
    let i = if nargs >= 3 {
        posrelat(
            vm.int_from(vm.nat_arg(fs, nargs, 2), "use as an index")?,
            len,
        )
    } else {
        default_i
    };
    if !(1..=len as i64 + 1).contains(&i) {
        // PUC 5.3 reads "position out of range"; 5.4 reworded to
        // "position out of bounds". utf8.lua bakes the matching substring.
        let suffix = if vm.version() <= crate::version::LuaVersion::Lua53 {
            "position out of range"
        } else {
            "position out of bounds"
        };
        return Err(arg_error(vm, 3, "offset", suffix));
    }
    let mut pos = (i - 1) as usize;
    let mut n = n;
    // PUC byteoffset: locate the n-th character start counting from `pos`.
    if n == 0 {
        // back up to the start of the byte sequence containing `pos`
        while pos > 0 && is_cont(s[pos]) {
            pos -= 1;
        }
    } else {
        if pos < len && is_cont(s[pos]) {
            return Err(raise_str(vm, "initial position is a continuation byte"));
        }
        if n < 0 {
            while n < 0 && pos > 0 {
                loop {
                    pos -= 1;
                    if !(pos > 0 && is_cont(s[pos])) {
                        break;
                    }
                }
                n += 1;
            }
        } else {
            n -= 1; // do not move for the 1st character
            while n > 0 && pos < len {
                loop {
                    pos += 1;
                    if !(pos < len && is_cont(s[pos])) {
                        break;
                    }
                }
                n -= 1;
            }
        }
    }
    if n != 0 {
        // did not find the requested character
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    // 5.5: return both the initial and the final byte position of the char
    let start = pos as i64 + 1;
    if pos < len && s[pos] & 0x80 != 0 {
        if is_cont(s[pos]) {
            return Err(raise_str(vm, "initial position is a continuation byte"));
        }
        while pos + 1 < len && is_cont(s[pos + 1]) {
            pos += 1;
        }
    }
    Ok(vm.nat_return(fs, &[Value::Int(start), Value::Int(pos as i64 + 1)]))
}

/// utf8.codes iterator: upvalues [s, lax].
fn codes_iter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(s) = vm.nat_arg(fs, nargs, 0) else {
        return Err(raise_str(vm, "bad iterator state for 'codes'"));
    };
    let prev = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(i) => i,
        _ => 0,
    };
    let lax = vm.nat_upval(fs, 0).truthy();
    let bytes = s.as_bytes().to_vec();
    // out-of-range control values end the iteration (PUC behavior)
    if prev < 0 || prev as usize > bytes.len() {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    // advance past the character at prev (or start)
    let mut pos = prev as usize;
    if pos > 0 {
        let Some((_, next)) = decode(&bytes, pos - 1, !lax) else {
            return Err(raise_str(vm, "invalid UTF-8 code"));
        };
        pos = next;
    }
    if pos >= bytes.len() {
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    let Some((cp, _)) = decode(&bytes, pos, !lax) else {
        return Err(raise_str(vm, "invalid UTF-8 code"));
    };
    Ok(vm.nat_return(fs, &[Value::Int(pos as i64 + 1), Value::Int(cp as i64)]))
}

fn u_codes(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = vm.nat_arg(fs, nargs, 0);
    let Value::Str(_) = s else {
        return Err(arg_error(
            vm,
            1,
            "codes",
            &format!("string expected, got {}", s.type_name()),
        ));
    };
    let lax = Value::Bool(vm.nat_arg(fs, nargs, 1).truthy());
    let it = vm.native_with(codes_iter, Box::new([lax]));
    Ok(vm.nat_return(fs, &[it, s, Value::Int(0)]))
}
