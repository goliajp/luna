//! string.pack / string.unpack / string.packsize — faithful port of the PUC
//! Lua 5.5 lstrlib.c pack engine. Binary (de)serialization of integers,
//! floats, and strings with explicit size / endianness / alignment control.
//!
//! Byte-level primitives (`pack_int` / `unpack_int` / float conversion) are
//! pure functions; the option parser and value plumbing live alongside the
//! string library since they are coupled to the VM's value/error model.

use crate::runtime::Value;
use crate::runtime::value::f2i_exact;
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::lib_string::check_str;

/// Maximum size for the binary representation of an integer.
const MAXINTSIZE: u64 = 16;
/// `sizeof(lua_Integer)`.
const SZINT: u64 = 8;
/// `MAX_SIZE`: bounds every `c`/`s` size and the accumulated result. PUC 5.4
/// caps it at `INT_MAX` (the conditional definition in lstrlib.c picks the
/// smaller of `size_t` and `int`); 5.5 dropped that cap so the limit is
/// `LUA_MAXINTEGER` on a 64-bit target. tpack.lua's "too large" probes are
/// version-specific: 5.4 fires at 2^31, 5.5 only at 2^63 — `max_size` returns
/// the right value for the calling Vm.
fn max_size(vm: &Vm) -> u64 {
    if vm.version() >= crate::version::LuaVersion::Lua55 {
        i64::MAX as u64
    } else {
        i32::MAX as u64
    }
}
/// Native endianness assumed by the `=` option (our targets are little).
const NATIVE_LITTLE: bool = true;
/// Native max alignment (`offsetof(struct cD, u)`): matches `lua_Number`/
/// `lua_Integer` (both 8). Only explicit `!N` settings are asserted by tests.
const NATIVE_MAXALIGN: u64 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum KOption {
    Int,
    Uint,
    Float,
    Number,
    Double,
    Char,
    Str,
    Zstr,
    Padding,
    PadAlign,
    Nop,
}

struct Header {
    islittle: bool,
    maxalign: u64,
}

impl Header {
    fn new() -> Self {
        Header {
            islittle: NATIVE_LITTLE,
            maxalign: 1,
        }
    }
}

fn digit(c: u8) -> bool {
    c.is_ascii_digit()
}

/// Read an integer numeral from `fmt[*pos..]`, or return `df` if none.
fn getnum(vm: &Vm, fmt: &[u8], pos: &mut usize, df: u64) -> u64 {
    if *pos >= fmt.len() || !digit(fmt[*pos]) {
        return df;
    }
    let cap = (max_size(vm) - 9) / 10;
    let mut a: u64 = 0;
    loop {
        a = a * 10 + (fmt[*pos] - b'0') as u64;
        *pos += 1;
        if !(*pos < fmt.len() && digit(fmt[*pos]) && a <= cap) {
            break;
        }
    }
    a
}

/// Read a numeral and error if it is not a legal integral size [1, MAXINTSIZE].
fn getnumlimit(vm: &mut Vm, fmt: &[u8], pos: &mut usize, df: u64) -> Result<u64, LuaError> {
    let sz = getnum(vm, fmt, pos, df);
    if sz.wrapping_sub(1) >= MAXINTSIZE {
        return Err(raise_str(
            vm,
            &format!("integral size ({sz}) out of limits [1,{MAXINTSIZE}]"),
        ));
    }
    Ok(sz)
}

/// Read and classify the next option; returns `(opt, size)`.
fn getoption(
    vm: &mut Vm,
    h: &mut Header,
    fmt: &[u8],
    pos: &mut usize,
) -> Result<(KOption, u64), LuaError> {
    let opt = fmt[*pos];
    *pos += 1;
    let mut size = 0u64;
    let k = match opt {
        b'b' => {
            size = 1;
            KOption::Int
        }
        b'B' => {
            size = 1;
            KOption::Uint
        }
        b'h' => {
            size = 2;
            KOption::Int
        }
        b'H' => {
            size = 2;
            KOption::Uint
        }
        b'l' | b'j' => {
            size = 8;
            KOption::Int
        }
        b'L' | b'J' => {
            size = 8;
            KOption::Uint
        }
        b'T' => {
            size = 8;
            KOption::Uint
        }
        b'f' => {
            size = 4;
            KOption::Float
        }
        b'n' => {
            size = 8;
            KOption::Number
        }
        b'd' => {
            size = 8;
            KOption::Double
        }
        b'i' => {
            size = getnumlimit(vm, fmt, pos, 4)?;
            KOption::Int
        }
        b'I' => {
            size = getnumlimit(vm, fmt, pos, 4)?;
            KOption::Uint
        }
        b's' => {
            size = getnumlimit(vm, fmt, pos, 8)?;
            KOption::Str
        }
        b'c' => {
            size = getnum(vm, fmt, pos, u64::MAX);
            if size == u64::MAX {
                return Err(raise_str(vm, "missing size for format option 'c'"));
            }
            KOption::Char
        }
        b'z' => KOption::Zstr,
        b'x' => {
            size = 1;
            KOption::Padding
        }
        b'X' => KOption::PadAlign,
        b' ' => KOption::Nop,
        b'<' => {
            h.islittle = true;
            KOption::Nop
        }
        b'>' => {
            h.islittle = false;
            KOption::Nop
        }
        b'=' => {
            h.islittle = NATIVE_LITTLE;
            KOption::Nop
        }
        b'!' => {
            h.maxalign = getnumlimit(vm, fmt, pos, NATIVE_MAXALIGN)?;
            KOption::Nop
        }
        _ => {
            return Err(raise_str(
                vm,
                &format!("invalid format option '{}'", opt as char),
            ));
        }
    };
    Ok((k, size))
}

/// Read, classify, and compute alignment padding for the next option.
fn getdetails(
    vm: &mut Vm,
    h: &mut Header,
    totalsize: u64,
    fmt: &[u8],
    pos: &mut usize,
    who: &str,
) -> Result<(KOption, u64, u64), LuaError> {
    let (opt, size) = getoption(vm, h, fmt, pos)?;
    let mut align = size;
    if opt == KOption::PadAlign {
        // 'X' takes its alignment from the following option, which it consumes.
        if *pos >= fmt.len() {
            return Err(arg_error(vm, 1, who, "invalid next option for option 'X'"));
        }
        let (nopt, nsize) = getoption(vm, h, fmt, pos)?;
        align = nsize;
        if nopt == KOption::Char || align == 0 {
            return Err(arg_error(vm, 1, who, "invalid next option for option 'X'"));
        }
    }
    let ntoalign = if align <= 1 || opt == KOption::Char {
        0
    } else {
        if align > h.maxalign {
            align = h.maxalign;
        }
        if align & (align - 1) != 0 {
            return Err(arg_error(
                vm,
                1,
                who,
                "format asks for alignment not power of 2",
            ));
        }
        let szmoda = totalsize & (align - 1);
        (align - szmoda) & (align - 1)
    };
    Ok((opt, size, ntoalign))
}

/// Pack `n` into `size` bytes (little-endian first, reversed for big), with
/// sign extension of the high bytes when `neg` and `size > SZINT`.
fn pack_int(out: &mut Vec<u8>, mut n: u64, islittle: bool, size: usize, neg: bool) {
    let mut bytes = vec![0u8; size];
    bytes[0] = (n & 0xff) as u8;
    for b in bytes.iter_mut().take(size).skip(1) {
        n >>= 8;
        *b = (n & 0xff) as u8;
    }
    if neg && size > SZINT as usize {
        for b in bytes.iter_mut().skip(SZINT as usize) {
            *b = 0xff;
        }
    }
    if !islittle {
        bytes.reverse();
    }
    out.extend_from_slice(&bytes);
}

/// Unpack a `size`-byte integer from `str[..size]`, sign-extending or checking
/// the high bytes as PUC does. `str` must hold at least `size` bytes.
fn unpack_int(
    vm: &mut Vm,
    str: &[u8],
    islittle: bool,
    size: usize,
    issigned: bool,
) -> Result<i64, LuaError> {
    let mut res: u64 = 0;
    let limit = if size <= SZINT as usize {
        size
    } else {
        SZINT as usize
    };
    for i in (0..limit).rev() {
        res <<= 8;
        let idx = if islittle { i } else { size - 1 - i };
        res |= str[idx] as u64;
    }
    if size < SZINT as usize {
        if issigned {
            let mask = 1u64 << (size * 8 - 1);
            res = (res ^ mask).wrapping_sub(mask);
        }
    } else if size > SZINT as usize {
        let fill: u8 = if !issigned || (res as i64) >= 0 {
            0
        } else {
            0xff
        };
        for i in limit..size {
            let idx = if islittle { i } else { size - 1 - i };
            if str[idx] != fill {
                return Err(raise_str(
                    vm,
                    &format!("{size}-byte integer does not fit into Lua Integer"),
                ));
            }
        }
    }
    Ok(res as i64)
}

fn check_int(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<i64, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Int(x) => Ok(x),
        Value::Float(f) => f2i_exact(f)
            .ok_or_else(|| arg_error(vm, i + 1, who, "number has no integer representation")),
        v => Err(arg_error(
            vm,
            i + 1,
            who,
            &format!("number expected, got {}", v.type_name()),
        )),
    }
}

fn check_num(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<f64, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Int(x) => Ok(x as f64),
        Value::Float(f) => Ok(f),
        v => Err(arg_error(
            vm,
            i + 1,
            who,
            &format!("number expected, got {}", v.type_name()),
        )),
    }
}

pub(crate) fn s_pack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = check_str(vm, fs, nargs, 0, "pack")?;
    let fmt = f.as_bytes().to_vec();
    let mut h = Header::new();
    let mut out: Vec<u8> = Vec::new();
    let mut totalsize: u64 = 0;
    let mut argi: u32 = 1; // 0-based arg index; 0 is the format
    let mut fp = 0usize;
    while fp < fmt.len() {
        let (opt, size, ntoalign) = getdetails(vm, &mut h, totalsize, &fmt, &mut fp, "pack")?;
        if size + ntoalign > max_size(vm).saturating_sub(totalsize) {
            return Err(arg_error(vm, argi + 1, "pack", "result too long"));
        }
        totalsize += ntoalign + size;
        out.resize(out.len() + ntoalign as usize, 0);
        match opt {
            KOption::Int => {
                let n = check_int(vm, fs, nargs, argi, "pack")?;
                if size < SZINT {
                    let lim = 1i64 << ((size * 8 - 1) as u32);
                    if !(-lim <= n && n < lim) {
                        return Err(arg_error(vm, argi + 1, "pack", "integer overflow"));
                    }
                }
                pack_int(&mut out, n as u64, h.islittle, size as usize, n < 0);
                argi += 1;
            }
            KOption::Uint => {
                let n = check_int(vm, fs, nargs, argi, "pack")?;
                if size < SZINT && (n as u64) >= (1u64 << ((size * 8) as u32)) {
                    return Err(arg_error(vm, argi + 1, "pack", "unsigned overflow"));
                }
                pack_int(&mut out, n as u64, h.islittle, size as usize, false);
                argi += 1;
            }
            KOption::Float => {
                let x = check_num(vm, fs, nargs, argi, "pack")? as f32;
                let mut b = x.to_le_bytes().to_vec();
                if !h.islittle {
                    b.reverse();
                }
                out.extend_from_slice(&b);
                argi += 1;
            }
            KOption::Number | KOption::Double => {
                let x = check_num(vm, fs, nargs, argi, "pack")?;
                let mut b = x.to_le_bytes().to_vec();
                if !h.islittle {
                    b.reverse();
                }
                out.extend_from_slice(&b);
                argi += 1;
            }
            KOption::Char => {
                let s = check_str(vm, fs, nargs, argi, "pack")?;
                let bytes = s.as_bytes().to_vec();
                let len = bytes.len() as u64;
                if len > size {
                    return Err(arg_error(
                        vm,
                        argi + 1,
                        "pack",
                        "string longer than given size",
                    ));
                }
                out.extend_from_slice(&bytes);
                out.resize(out.len() + (size - len) as usize, 0);
                argi += 1;
            }
            KOption::Str => {
                let s = check_str(vm, fs, nargs, argi, "pack")?;
                let bytes = s.as_bytes().to_vec();
                let len = bytes.len() as u64;
                if !(size >= SZINT || len < (1u64 << ((size * 8) as u32))) {
                    return Err(arg_error(
                        vm,
                        argi + 1,
                        "pack",
                        "string length does not fit in given size",
                    ));
                }
                pack_int(&mut out, len, h.islittle, size as usize, false);
                out.extend_from_slice(&bytes);
                totalsize += len;
                argi += 1;
            }
            KOption::Zstr => {
                let s = check_str(vm, fs, nargs, argi, "pack")?;
                let bytes = s.as_bytes().to_vec();
                if bytes.contains(&0) {
                    return Err(arg_error(vm, argi + 1, "pack", "string contains zeros"));
                }
                out.extend_from_slice(&bytes);
                out.push(0);
                totalsize += bytes.len() as u64 + 1;
                argi += 1;
            }
            KOption::Padding => out.push(0),
            KOption::PadAlign | KOption::Nop => {}
        }
    }
    let v = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[v]))
}

pub(crate) fn s_packsize(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = check_str(vm, fs, nargs, 0, "packsize")?;
    let fmt = f.as_bytes().to_vec();
    let mut h = Header::new();
    let mut totalsize: u64 = 0;
    let mut fp = 0usize;
    while fp < fmt.len() {
        let (opt, size, ntoalign) = getdetails(vm, &mut h, totalsize, &fmt, &mut fp, "packsize")?;
        if opt == KOption::Str || opt == KOption::Zstr {
            return Err(arg_error(vm, 1, "packsize", "variable-length format"));
        }
        let need = size + ntoalign;
        let cap = max_size(vm);
        if need > cap || totalsize > cap - need {
            return Err(arg_error(vm, 1, "packsize", "format result too large"));
        }
        totalsize += need;
    }
    Ok(vm.nat_return(fs, &[Value::Int(totalsize as i64)]))
}

/// PUC posrelatI: translate the optional 1-based/negative initial position.
fn posrelat_i(pos: i64, len: u64) -> u64 {
    if pos > 0 {
        pos as u64
    } else if pos == 0 || pos < -(len as i64) {
        1 // zero, or a negative index past the start, clips to 1
    } else {
        (len as i64 + pos + 1) as u64
    }
}

pub(crate) fn s_unpack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = check_str(vm, fs, nargs, 0, "unpack")?;
    let fmt = f.as_bytes().to_vec();
    let d = check_str(vm, fs, nargs, 1, "unpack")?;
    let data = d.as_bytes().to_vec();
    let ld = data.len() as u64;
    let initpos = match vm.nat_arg(fs, nargs, 2) {
        Value::Nil => 1i64,
        v => vm.int_from(v, "use as an index")?,
    };
    // PUC ≤5.3 `string.unpack` is strict: `pos` must be in `[1, #s + 1]`
    // (positive) or `[-#s, -1]` (negative — counted from the end). 5.4+
    // silently clamps zero/negative indices via `posrelat`. tpack.lua 5.3
    // :316 probes `pos = 0` and `pos = -(#x + 1)` and expects the error.
    if vm.version() <= crate::version::LuaVersion::Lua53 {
        let in_range = if initpos > 0 {
            (initpos as u64) <= ld + 1
        } else if initpos < 0 {
            (-initpos as u64) <= ld
        } else {
            false
        };
        if !in_range {
            return Err(arg_error(vm, 3, "unpack", "initial position out of string"));
        }
    }
    let mut pos = posrelat_i(initpos, ld) - 1;
    if pos > ld {
        return Err(arg_error(vm, 3, "unpack", "initial position out of string"));
    }
    let mut h = Header::new();
    let mut results: Vec<Value> = Vec::new();
    let mut fp = 0usize;
    while fp < fmt.len() {
        let (opt, size, ntoalign) = getdetails(vm, &mut h, pos, &fmt, &mut fp, "unpack")?;
        if ntoalign + size > ld - pos {
            return Err(arg_error(vm, 2, "unpack", "data string too short"));
        }
        pos += ntoalign;
        let p = pos as usize;
        match opt {
            KOption::Int | KOption::Uint => {
                let res = unpack_int(
                    vm,
                    &data[p..],
                    h.islittle,
                    size as usize,
                    opt == KOption::Int,
                )?;
                results.push(Value::Int(res));
            }
            KOption::Float => {
                let mut b = data[p..p + 4].to_vec();
                if !h.islittle {
                    b.reverse();
                }
                let arr: [u8; 4] = b.try_into().expect("4 bytes");
                results.push(Value::Float(f32::from_le_bytes(arr) as f64));
            }
            KOption::Number | KOption::Double => {
                let mut b = data[p..p + 8].to_vec();
                if !h.islittle {
                    b.reverse();
                }
                let arr: [u8; 8] = b.try_into().expect("8 bytes");
                results.push(Value::Float(f64::from_le_bytes(arr)));
            }
            KOption::Char => {
                let s = vm.heap.intern(&data[p..p + size as usize]);
                results.push(Value::Str(s));
            }
            KOption::Str => {
                let len = unpack_int(vm, &data[p..], h.islittle, size as usize, false)? as u64;
                if len > ld - pos - size {
                    return Err(arg_error(vm, 2, "unpack", "data string too short"));
                }
                let st = (pos + size) as usize;
                let s = vm.heap.intern(&data[st..st + len as usize]);
                results.push(Value::Str(s));
                pos += len;
            }
            KOption::Zstr => match data[p..].iter().position(|&b| b == 0) {
                Some(len) => {
                    let s = vm.heap.intern(&data[p..p + len]);
                    results.push(Value::Str(s));
                    pos += len as u64 + 1;
                }
                None => {
                    return Err(arg_error(
                        vm,
                        2,
                        "unpack",
                        "unfinished string for format 'z'",
                    ));
                }
            },
            KOption::Padding | KOption::PadAlign | KOption::Nop => {}
        }
        pos += size;
    }
    results.push(Value::Int((pos + 1) as i64));
    Ok(vm.nat_return(fs, &results))
}

#[cfg(test)]
mod tests {
    use crate::runtime::Value;
    use crate::version::LuaVersion;
    use crate::vm::Vm;

    fn run(src: &str) -> Result<Vec<Value>, String> {
        let mut vm = Vm::new(LuaVersion::Lua55);
        let cl = vm
            .load(src.as_bytes(), b"@test")
            .map_err(|e| e.to_string())?;
        vm.call_value(Value::Closure(cl), &[])
            .map_err(|e| vm.error_text(&e))
    }

    #[test]
    fn int_roundtrip_endianness() {
        run(r#"
            assert(string.unpack("B", string.pack("B", 0xff)) == 0xff)
            assert(string.unpack("<i4", string.pack("<i4", -1)) == -1)
            assert(string.unpack(">i4", string.pack(">i4", -1)) == -1)
            assert(string.pack("<i2", 1) == "\1\0")
            assert(string.pack(">i2", 1) == "\0\1")
            assert(string.pack("<I3", 0xAA) == "\xAA\0\0")
        "#)
        .unwrap();
    }

    #[test]
    fn packsize_and_variable_errors() {
        run(r#"
            assert(string.packsize("i4") == 4)
            assert(string.packsize("<! c3") == 3)
            assert(string.packsize("!8 xXi8") == 8)
            local ok = pcall(string.packsize, "s")
            assert(not ok)
            local ok2 = pcall(string.packsize, "z")
            assert(not ok2)
        "#)
        .unwrap();
    }

    #[test]
    fn strings_and_floats() {
        run(r#"
            local s = "alo"
            assert(string.unpack("z", string.pack("z", s)) == s)
            assert(string.unpack("s4", string.pack("s4", s)) == s)
            assert(string.unpack("n", string.pack("n", 1.5)) == 1.5)
            assert(string.pack("<f", 24) == string.pack(">f", 24):reverse())
            assert(string.pack("c8", "123456") == "123456\0\0")
        "#)
        .unwrap();
    }

    #[test]
    fn overflow_and_fit_errors() {
        run(r#"
            assert(not pcall(string.pack, "<I1", -1))      -- unsigned overflow
            assert(not pcall(string.pack, ">i1", 0xFF))    -- integer overflow
            assert(not pcall(string.pack, "i0", 0))        -- out of limits
            assert(not pcall(string.pack, "i17", 0))       -- out of limits
            assert(not pcall(string.pack, "c3", "1234"))   -- longer than
            assert(not pcall(string.unpack, "i16", string.rep("\3", 16))) -- does not fit
        "#)
        .unwrap();
    }
}
