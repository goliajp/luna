//! Format-string parsing: options, sizes and alignment.

use super::*;

/// PUC's size limit: `INT_MAX` through 5.4 (`MAXSIZE`), `LUA_MAXINTEGER`
/// in 5.5 (`MAX_SIZE`). It bounds numerals in formats and the packsize.
pub(super) fn max_size(vm: &Vm) -> u64 {
    if vm.version() >= LuaVersion::Lua55 {
        i64::MAX as u64
    } else {
        i32::MAX as u64
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum KOption {
    Int,
    Uint,
    Float,
    Number,
    Char,
    Str,
    Zstr,
    Padding,
    PadAlign,
    Nop,
}

pub(super) struct Header {
    pub(super) islittle: bool,
    maxalign: u64,
}

impl Header {
    pub(super) fn new() -> Self {
        Header {
            islittle: NATIVE_LITTLE,
            maxalign: 1,
        }
    }
}

/// Read an integer numeral from `fmt[*pos..]`, or return `df` if none.
fn getnum(vm: &Vm, fmt: &[u8], pos: &mut usize, df: u64) -> u64 {
    if !fmt.get(*pos).is_some_and(u8::is_ascii_digit) {
        return df;
    }
    let cap = (max_size(vm) - 9) / 10;
    let mut a: u64 = 0;
    loop {
        a = a * 10 + u64::from(fmt[*pos] - b'0');
        *pos += 1;
        if !(fmt.get(*pos).is_some_and(u8::is_ascii_digit) && a <= cap) {
            return a;
        }
    }
}

/// Read a numeral and error if it is not a legal integral size [1, 16].
fn getnumlimit(vm: &mut Vm, fmt: &[u8], pos: &mut usize, df: u64) -> Result<u64, LuaError> {
    let sz = getnum(vm, fmt, pos, df);
    if sz.wrapping_sub(1) >= MAXINTSIZE {
        // printed with "%d": 5.5's size_t shows its low 32 bits
        let shown = sz as u32 as i32;
        return Err(raise_str(
            vm,
            &format!("integral size ({shown}) out of limits [1,{MAXINTSIZE}]"),
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
    Ok(match opt {
        b'b' => (KOption::Int, 1),
        b'B' => (KOption::Uint, 1),
        b'h' => (KOption::Int, 2),
        b'H' => (KOption::Uint, 2),
        b'l' | b'j' => (KOption::Int, 8),
        b'L' | b'J' | b'T' => (KOption::Uint, 8),
        b'f' => (KOption::Float, 4),
        b'n' | b'd' => (KOption::Number, 8),
        b'i' => (KOption::Int, getnumlimit(vm, fmt, pos, 4)?),
        b'I' => (KOption::Uint, getnumlimit(vm, fmt, pos, 4)?),
        b's' => (KOption::Str, getnumlimit(vm, fmt, pos, 8)?),
        b'c' => {
            let size = getnum(vm, fmt, pos, u64::MAX);
            if size == u64::MAX {
                return Err(raise_str(vm, "missing size for format option 'c'"));
            }
            (KOption::Char, size)
        }
        b'z' => (KOption::Zstr, 0),
        b'x' => (KOption::Padding, 1),
        b'X' => (KOption::PadAlign, 0),
        b' ' => (KOption::Nop, 0),
        b'<' => {
            h.islittle = true;
            (KOption::Nop, 0)
        }
        b'>' => {
            h.islittle = false;
            (KOption::Nop, 0)
        }
        b'=' => {
            h.islittle = NATIVE_LITTLE;
            (KOption::Nop, 0)
        }
        b'!' => {
            h.maxalign = getnumlimit(vm, fmt, pos, NATIVE_MAXALIGN)?;
            (KOption::Nop, 0)
        }
        _ => {
            let mut msg = b"invalid format option '".to_vec();
            msg.push(opt);
            msg.push(b'\'');
            return Err(crate::vm::builtins::raise_bytes(vm, &msg));
        }
    })
}

/// Read, classify, and compute alignment padding for the next option.
pub(super) fn getdetails(
    vm: &mut Vm,
    h: &mut Header,
    totalsize: u64,
    fmt: &[u8],
    pos: &mut usize,
) -> Result<(KOption, u64, u64), LuaError> {
    let (opt, size) = getoption(vm, h, fmt, pos)?;
    let mut align = size;
    if opt == KOption::PadAlign {
        // 'X' takes its alignment from the following option, which it consumes
        let bad = *pos >= fmt.len() || {
            let (next, nsize) = getoption(vm, h, fmt, pos)?;
            align = nsize;
            next == KOption::Char || align == 0
        };
        if bad {
            return Err(arg_error(vm, 1, "invalid next option for option 'X'"));
        }
    }
    if align <= 1 || opt == KOption::Char {
        return Ok((opt, size, 0));
    }
    let align = align.min(h.maxalign);
    if align & (align - 1) != 0 {
        return Err(arg_error(vm, 1, "format asks for alignment not power of 2"));
    }
    Ok((opt, size, (align - (totalsize & (align - 1))) & (align - 1)))
}
