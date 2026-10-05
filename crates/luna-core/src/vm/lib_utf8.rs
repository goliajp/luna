//! utf8 library (5.3+), ported from each version's lutf8lib.c. Lua's UTF-8
//! is byte-oriented; 5.3 decodes up to U+10FFFF, 5.4 extends sequences to
//! 2^31 and adds the `lax` flag that decides whether surrogates and code
//! points past U+10FFFF are accepted.

use crate::runtime::Value;
use crate::runtime::mem::LVec;
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_utf8(vm: &mut Vm) {
    crate::vm::lib_mem::opened(try_open_utf8(vm));
}

/// [`open_utf8`], failing when memory runs out.
pub(crate) fn try_open_utf8(vm: &mut Vm) -> Result<(), LuaError> {
    let v = vm.version();
    if v < LuaVersion::Lua53 {
        return Ok(());
    }
    let t = vm.alloc_table()?;
    let set = |vm: &mut Vm, name: &str, fv: Value| -> Result<(), LuaError> {
        let k = vm.str_value(name.as_bytes())?;
        vm.lib_set_value(t, k, fv)
    };
    let fns: [(&str, fn(&mut Vm, u32, u32) -> Result<u32, LuaError>); 5] = [
        ("char", u_char),
        ("codepoint", u_codepoint),
        ("len", u_len),
        ("offset", u_offset),
        ("codes", u_codes),
    ];
    for (name, f) in fns {
        let fv = vm.lib_native(f)?;
        set(vm, name, fv)?;
    }
    let pattern: &[u8] = if v == LuaVersion::Lua53 {
        b"[\x00-\x7F\xC2-\xF4][\x80-\xBF]*"
    } else {
        b"[\x00-\x7F\xC2-\xFD][\x80-\xBF]*"
    };
    let p = vm.str_value(pattern)?;
    set(vm, "charpattern", p)?;
    vm.lib_set_global("utf8", Value::Table(t))?;
    vm.barrier_back_table(t);
    Ok(())
}

const MAX_UNICODE: u32 = 0x10_FFFF;
const MAX_UTF: u32 = 0x7FFF_FFFF;

fn is_cont(b: u8) -> bool {
    b & 0xC0 == 0x80
}

/// The byte at `i`, or the terminating zero past the end.
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// `utf8_decode`: the code point starting at `i` and the index after it,
/// or `None` for an invalid sequence. `strict` is the 5.4 check against
/// surrogates and values past U+10FFFF.
fn decode(v: LuaVersion, s: &[u8], i: usize, strict: bool) -> Option<(u32, usize)> {
    let mut c = u32::from(at(s, i));
    if c < 0x80 {
        return Some((c, i + 1));
    }
    let legacy = v == LuaVersion::Lua53;
    if !legacy && c >= 0xFE {
        return None;
    }
    let mut res: u32 = 0;
    let mut count = 0;
    while c & 0x40 != 0 {
        count += 1;
        let cc = at(s, i + count);
        if !is_cont(cc) {
            return None;
        }
        res = (res << 6) | u32::from(cc & 0x3F);
        c <<= 1;
    }
    // 5.3 reads up to seven continuation bytes before rejecting more than
    // three
    if legacy && count > 3 {
        return None;
    }
    res |= (c & 0x7F) << (count * 5);
    if legacy {
        const LIMITS: [u32; 4] = [0xFF, 0x7F, 0x7FF, 0xFFFF];
        if res > MAX_UNICODE || res <= LIMITS[count] {
            return None;
        }
    } else {
        const LIMITS: [u32; 6] = [u32::MAX, 0x80, 0x800, 0x10000, 0x20_0000, 0x400_0000];
        if res > MAX_UTF || res < LIMITS[count] {
            return None;
        }
        if strict && (res > MAX_UNICODE || (0xD800..=0xDFFF).contains(&res)) {
            return None;
        }
    }
    Some((res, i + count + 1))
}

/// `luaO_utf8esc`.
fn encode(vm: &mut Vm, out: &mut LVec<u8>, mut x: u32) -> Result<(), LuaError> {
    if x < 0x80 {
        return vm.buf_push(out, &[x as u8]);
    }
    let mut cont = [0u8; 6];
    let mut n = 0;
    let mut mfb: u32 = 0x3f;
    loop {
        cont[n] = 0x80 | (x & 0x3f) as u8;
        n += 1;
        x >>= 6;
        mfb >>= 1;
        if x <= mfb {
            break;
        }
    }
    let mut seq = [0u8; 7];
    seq[0] = ((!mfb << 1) | x) as u8;
    for (k, &c) in cont[..n].iter().rev().enumerate() {
        seq[k + 1] = c;
    }
    vm.buf_push(out, &seq[..=n])
}

fn u_posrelat(pos: i64, len: usize) -> i64 {
    if pos >= 0 {
        pos
    } else if pos.unsigned_abs() > len as u64 {
        0
    } else {
        len as i64 + pos + 1
    }
}

/// The 5.3 → 5.4 rewording of the range errors.
fn bounds(v: LuaVersion, old: &'static str, new: &'static str) -> &'static str {
    if v == LuaVersion::Lua53 { old } else { new }
}

fn u_char(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let mut out = vm.new_buf();
    for i in 0..nargs {
        let code = argcheck::check_integer(vm, a, i)?;
        let ok = if v == LuaVersion::Lua53 {
            (0..=i64::from(MAX_UNICODE)).contains(&code)
        } else {
            (code as u64) <= u64::from(MAX_UTF)
        };
        if !ok {
            return Err(arg_error(vm, i + 1, "value out of range"));
        }
        encode(vm, &mut out, code as u32)?;
    }
    let s = vm.str_value(&out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[s])
}

fn u_codepoint(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let s = argcheck::check_string(vm, a, 0)?;
    let len = s.len();
    let posi = u_posrelat(argcheck::opt_integer(vm, a, 1, 1)?, len);
    let pose = u_posrelat(argcheck::opt_integer(vm, a, 2, posi)?, len);
    let lax = v >= LuaVersion::Lua54 && a.get(vm, 3).truthy();
    if posi < 1 {
        return Err(arg_error(vm, 2, bounds(v, "out of range", "out of bounds")));
    }
    if pose > len as i64 {
        return Err(arg_error(vm, 3, bounds(v, "out of range", "out of bounds")));
    }
    if posi > pose {
        return Ok(0);
    }
    argcheck::check_stack(vm, a, pose - posi + 1, "string slice too long")?;
    let bytes = s.as_bytes();
    let mut out = LVec::new(vm.heap.mem());
    let mut i = (posi - 1) as usize;
    while i < pose as usize {
        let Some((code, next)) = decode(v, bytes, i, !lax) else {
            return Err(raise_str(vm, "invalid UTF-8 code"));
        };
        out.push(Value::Int(i64::from(code)))?;
        i = next;
    }
    vm.nat_ret(fs, &out)
}

fn u_len(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let s = argcheck::check_string(vm, a, 0)?;
    let len = s.len() as i64;
    let posi = u_posrelat(argcheck::opt_integer(vm, a, 1, 1)?, s.len());
    let posj = u_posrelat(argcheck::opt_integer(vm, a, 2, -1)?, s.len());
    let lax = v >= LuaVersion::Lua54 && a.get(vm, 3).truthy();
    if !(1 <= posi && posi - 1 <= len) {
        let msg = bounds(
            v,
            "initial position out of string",
            "initial position out of bounds",
        );
        return Err(arg_error(vm, 2, msg));
    }
    if posj > len {
        let msg = bounds(
            v,
            "final position out of string",
            "final position out of bounds",
        );
        return Err(arg_error(vm, 3, msg));
    }
    let bytes = s.as_bytes();
    let (mut i, j) = (posi - 1, posj - 1);
    let mut n: i64 = 0;
    while i <= j {
        match decode(v, bytes, i as usize, !lax) {
            Some((_, next)) => {
                i = next as i64;
                n += 1;
            }
            None => return vm.nat_ret(fs, &[Value::Nil, Value::Int(i + 1)]),
        }
    }
    vm.nat_ret(fs, &[Value::Int(n)])
}

fn u_offset(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let s = argcheck::check_string(vm, a, 0)?;
    let len = s.len() as i64;
    let mut n = argcheck::check_integer(vm, a, 1)?;
    let default = if n >= 0 { 1 } else { len + 1 };
    let posi = u_posrelat(argcheck::opt_integer(vm, a, 2, default)?, s.len());
    if !(1 <= posi && posi - 1 <= len) {
        let msg = bounds(v, "position out of range", "position out of bounds");
        return Err(arg_error(vm, 3, msg));
    }
    let bytes = s.as_bytes();
    let cont = |i: i64| is_cont(at(bytes, i as usize));
    let mut posi = posi - 1;
    if n == 0 {
        // back up to the start of the sequence holding `posi`
        while posi > 0 && cont(posi) {
            posi -= 1;
        }
    } else {
        if cont(posi) {
            return Err(raise_str(vm, "initial position is a continuation byte"));
        }
        if n < 0 {
            while n < 0 && posi > 0 {
                loop {
                    posi -= 1;
                    if !(posi > 0 && cont(posi)) {
                        break;
                    }
                }
                n += 1;
            }
        } else {
            n -= 1; // the first character is where we already are
            while n > 0 && posi < len {
                loop {
                    posi += 1;
                    if !cont(posi) {
                        break;
                    }
                }
                n -= 1;
            }
        }
    }
    if n != 0 {
        return vm.nat_ret(fs, &[Value::Nil]);
    }
    let start = Value::Int(posi + 1);
    if v < LuaVersion::Lua55 {
        return vm.nat_ret(fs, &[start]);
    }
    // 5.5 also returns where the character ends
    if at(bytes, posi as usize) & 0x80 != 0 {
        if cont(posi) {
            return Err(raise_str(vm, "initial position is a continuation byte"));
        }
        while cont(posi + 1) {
            posi += 1;
        }
    }
    vm.nat_ret(fs, &[start, Value::Int(posi + 1)])
}

/// The `utf8.codes` iterator; upvalue [strict].
fn codes_iter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
            vm.nat_ret(fs, &[Value::Int(n as i64 + 1), Value::Int(i64::from(code))])
        }
        _ => Err(raise_str(vm, "invalid UTF-8 code")),
    }
}

fn u_codes(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let lax = v >= LuaVersion::Lua54 && a.get(vm, 1).truthy();
    let s = argcheck::check_string(vm, a, 0)?;
    if v >= LuaVersion::Lua54 && is_cont(at(s.as_bytes(), 0)) {
        return Err(arg_error(vm, 1, "invalid UTF-8 code"));
    }
    let it = vm.lib_native_with(codes_iter, &[Value::Bool(!lax)])?;
    vm.nat_ret(fs, &[it, Value::Str(s), Value::Int(0)])
}
