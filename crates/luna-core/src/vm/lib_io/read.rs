//! Read formats and the readers for lines, counts and the whole file.

use super::*;

/// Outcome of `g_read`: the values (the last one nil when a format failed),
/// or the I/O error that ends a read (`ferror`).
pub(super) enum ReadOut {
    Values(Vec<Value>),
    Error(std::io::Error),
}

/// One read format, parsed.
enum Fmt {
    Count(i64),
    Number,
    Line { keep_nl: bool },
    All,
}

/// Parse read format `fmt`, the argument numbered `argno` in errors.
fn parse_format(vm: &mut Vm, fmt: Value, argno: u32) -> Result<Fmt, LuaError> {
    let v = vm.version();
    match fmt {
        Value::Int(n) => return Ok(Fmt::Count(n)),
        Value::Float(f) if v <= LuaVersion::Lua52 => return Ok(Fmt::Count(f as i64)),
        Value::Float(f) => {
            return f2i_exact(f)
                .map(Fmt::Count)
                .ok_or_else(|| arg_error(vm, argno, "number has no integer representation"));
        }
        _ => {}
    }
    let spec = match fmt {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ if v <= LuaVersion::Lua52 => return Err(arg_error(vm, argno, "invalid option")),
        _ => {
            let tn = argcheck::typename_of(vm, fmt);
            return Err(arg_error(vm, argno, &format!("string expected, got {tn}")));
        }
    };
    // ≤5.2 require the '*'; 5.3 made it optional
    let body = match spec.strip_prefix(b"*") {
        Some(b) => b,
        None if v <= LuaVersion::Lua52 => return Err(arg_error(vm, argno, "invalid option")),
        None => &spec,
    };
    Ok(match body.first() {
        Some(b'n') => Fmt::Number,
        Some(b'l') => Fmt::Line { keep_nl: false },
        Some(b'L') if v >= LuaVersion::Lua52 => Fmt::Line { keep_nl: true },
        Some(b'a') => Fmt::All,
        _ => return Err(arg_error(vm, argno, "invalid format")),
    })
}

/// `g_read`: apply `fmts` in order until one fails. With no formats, read a
/// line. `argno0` numbers the first format in argument errors.
pub(super) fn g_read(
    vm: &mut Vm,
    u: Gc<Userdata>,
    fmts: &[Value],
    argno0: u32,
) -> Result<ReadOut, LuaError> {
    // stdio needs a flush between writing and reading the same stream
    if let Err(e) = drain_write_buf(u) {
        return Ok(ReadOut::Error(e));
    }
    if fmts.is_empty() {
        return Ok(match read_line(vm, u, false) {
            Ok(v) => ReadOut::Values(vec![v]),
            Err(e) => ReadOut::Error(e),
        });
    }
    let mut out = Vec::with_capacity(fmts.len());
    for (i, &f) in fmts.iter().enumerate() {
        let fmt = parse_format(vm, f, argno0 + i as u32)?;
        let r = match fmt {
            Fmt::Count(n) => read_count(vm, u, n)?,
            Fmt::Number => read_number(vm, u),
            Fmt::Line { keep_nl } => read_line(vm, u, keep_nl),
            Fmt::All => read_all(vm, u),
        };
        match r {
            Ok(v) => {
                let stop = v.is_nil();
                out.push(v);
                if stop {
                    break;
                }
            }
            Err(e) => return Ok(ReadOut::Error(e)),
        }
    }
    Ok(ReadOut::Values(out))
}

fn push_read(vm: &mut Vm, fs: u32, r: ReadOut) -> u32 {
    match r {
        ReadOut::Values(vals) => vm.nat_return(fs, &vals),
        ReadOut::Error(e) => file_fail(vm, fs, None, &e),
    }
}

pub(super) fn io_read(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = get_io_file(vm, Io::Input)?;
    let fmts: Vec<Value> = (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let r = g_read(vm, u, &fmts, 1)?;
    Ok(push_read(vm, fs, r))
}

pub(super) fn f_read(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    let fmts: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let r = g_read(vm, u, &fmts, 2)?;
    Ok(push_read(vm, fs, r))
}

/// `BUFSIZ`, the size of the chunks ≤5.2 read a line in (`LUAL_BUFFERSIZE`).
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
const BUFSIZ: usize = 1024;
#[cfg(windows)]
const BUFSIZ: usize = 512;
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
)))]
const BUFSIZ: usize = 8192;

/// `read_line`: up to and excluding (`keep_nl`: including) the newline; nil
/// when nothing at all was read.
fn read_line(vm: &mut Vm, u: Gc<Userdata>, keep_nl: bool) -> std::io::Result<Value> {
    if vm.version() <= LuaVersion::Lua52 {
        return read_line_fgets(vm, u, keep_nl);
    }
    let mut buf = Vec::new();
    let mut got_nl = false;
    while let Some(c) = getc(u)? {
        if c == b'\n' {
            got_nl = true;
            if keep_nl {
                buf.push(c);
            }
            break;
        }
        buf.push(c);
    }
    if got_nl || !buf.is_empty() {
        read_str(vm, &buf)
    } else {
        Ok(Value::Nil)
    }
}

/// ≤5.2's `read_line` reads with `fgets` and measures each chunk with
/// `strlen`: a NUL cuts the chunk short there, and a newline after it is
/// lost, so the line runs on into the next.
fn read_line_fgets(vm: &mut Vm, u: Gc<Userdata>, keep_nl: bool) -> std::io::Result<Value> {
    let mut out = Vec::new();
    loop {
        let mut chunk = Vec::new();
        while chunk.len() < BUFSIZ - 1 {
            match getc(u)? {
                Some(c) => {
                    chunk.push(c);
                    if c == b'\n' {
                        break;
                    }
                }
                None => break,
            }
        }
        if chunk.is_empty() {
            return if out.is_empty() {
                Ok(Value::Nil)
            } else {
                read_str(vm, &out)
            };
        }
        // strlen: up to the first NUL, the whole chunk when there is none
        let len = chunk.iter().position(|&b| b == 0).unwrap_or(chunk.len());
        if len == 0 || chunk[len - 1] != b'\n' {
            out.extend_from_slice(&chunk[..len]);
        } else {
            let end = if keep_nl { len } else { len - 1 };
            out.extend_from_slice(&chunk[..end]);
            return read_str(vm, &out);
        }
    }
}

/// A string read from a file. One longer than a string can hold is an
/// allocation failure, which PUC's buffer would meet first.
fn read_str(vm: &mut Vm, bytes: &[u8]) -> std::io::Result<Value> {
    if bytes.len() > crate::runtime::string::MAX_LEN {
        return Err(posix_error(ENOMEM));
    }
    Ok(Value::Str(vm.heap.intern(bytes)))
}

/// `read_all`: never fails (an empty string at end of file).
fn read_all(vm: &mut Vm, u: Gc<Userdata>) -> std::io::Result<Value> {
    if u.text.is_some() {
        let buf = read_all_text(vm.version(), u)?;
        return read_str(vm, &buf);
    }
    let mut buf = Vec::new();
    loop {
        buf.extend_from_slice(&u.read_buf[u.read_pos..]);
        // SAFETY: `u` is a file handle the caller holds; the right-hand side is read before the borrow starts, and the borrow covers one field store
        unsafe { u.as_mut() }.read_pos = u.read_buf.len();
        if !fill(u)? {
            break;
        }
    }
    read_str(vm, &buf)
}

/// `LUAL_BUFFERSIZE` of PUC built for 64-bit Windows.
fn lual_buffersize(v: LuaVersion) -> usize {
    match v {
        LuaVersion::Lua51 | LuaVersion::Lua52 => 512,
        LuaVersion::Lua53 => 8192,
        _ => 1024,
    }
}

/// Each dialect's `read_all` over a text mode stream, whose `fread` calls
/// decide what stays in the stream buffer (and so what `seek` reports).
fn read_all_text(v: LuaVersion, u: Gc<Userdata>) -> std::io::Result<Vec<u8>> {
    if v == LuaVersion::Lua51 {
        return read_chars_51(u, usize::MAX);
    }
    let mut rlen = lual_buffersize(v);
    let mut out = Vec::new();
    loop {
        let got = text_mode::fread(u, rlen)?;
        let short = got.len() < rlen;
        out.extend_from_slice(&got);
        if short {
            return Ok(out);
        }
        // 5.2 doubles its buffer on every round
        if v == LuaVersion::Lua52 {
            rlen *= 2;
        }
    }
}

/// 5.1's `read_chars`: `fread` in `LUAL_BUFFERSIZE` chunks.
fn read_chars_51(u: Gc<Userdata>, mut n: usize) -> std::io::Result<Vec<u8>> {
    let mut rlen = lual_buffersize(LuaVersion::Lua51);
    let mut out = Vec::new();
    loop {
        rlen = rlen.min(n);
        let got = text_mode::fread(u, rlen)?;
        n -= got.len();
        let full = got.len() == rlen;
        out.extend_from_slice(&got);
        if n == 0 || !full {
            return Ok(out);
        }
    }
}

/// Sizes no allocator grants; PUC's buffer for them fails before reading.
const UNALLOCATABLE: u64 = 1 << 47;

/// A byte-count format: `0` tests for end of file; `n` reads up to `n`
/// bytes (nil when none were left). A negative count is a huge `size_t`.
fn read_count(vm: &mut Vm, u: Gc<Userdata>, n: i64) -> Result<std::io::Result<Value>, LuaError> {
    let size = n as u64;
    if size == 0 {
        return Ok(test_eof(vm, u));
    }
    // 5.1 reads in chunks, so any size works; 5.2+ size one buffer for the
    // whole request, which the allocator refuses for absurd sizes
    if size >= UNALLOCATABLE && vm.version() >= LuaVersion::Lua52 {
        return Err(match vm.version() {
            LuaVersion::Lua52 if size > u64::MAX - 64 => {
                vm.plain_err("memory allocation error: block too big")
            }
            LuaVersion::Lua53 => raise_str(vm, "not enough memory for buffer allocation"),
            LuaVersion::Lua55 if size >= i64::MAX as u64 => {
                raise_str(vm, "resulting string too large")
            }
            _ => vm.plain_err("not enough memory"),
        });
    }
    if u.text.is_some() {
        let got = if vm.version() == LuaVersion::Lua51 {
            read_chars_51(u, size.min(usize::MAX as u64) as usize)
        } else {
            text_mode::fread(u, size as usize)
        };
        return Ok(match got {
            Ok(b) if b.is_empty() => Ok(Value::Nil),
            Ok(b) => read_str(vm, &b),
            Err(e) => Err(e),
        });
    }
    let mut buf = Vec::new();
    while (buf.len() as u64) < size {
        let want = (size - buf.len() as u64) as usize;
        if u.read_pos >= u.read_buf.len() {
            match fill(u) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => return Ok(Err(e)),
            }
        }
        let take = want.min(u.read_buf.len() - u.read_pos);
        buf.extend_from_slice(&u.read_buf[u.read_pos..u.read_pos + take]);
        // SAFETY: `u` is a file handle the caller holds; the borrow covers one field store
        unsafe { u.as_mut() }.read_pos += take;
    }
    Ok(if buf.is_empty() {
        Ok(Value::Nil)
    } else {
        read_str(vm, &buf)
    })
}

/// `test_eof`: "" if a byte is left, nil at end of file.
fn test_eof(vm: &mut Vm, u: Gc<Userdata>) -> std::io::Result<Value> {
    Ok(match getc(u)? {
        Some(c) => {
            unget(u, &[c]);
            Value::Str(vm.heap.intern(b""))
        }
        None => Value::Nil,
    })
}

fn read_number(vm: &mut Vm, u: Gc<Userdata>) -> std::io::Result<Value> {
    if vm.version() <= LuaVersion::Lua52 {
        return scan_double(u);
    }
    let buf = read_numeral(u)?;
    Ok(match numeric::str2num(&buf, true, true) {
        Some(Num::Int(i)) => Value::Int(i),
        Some(Num::Float(f)) => Value::Float(f),
        None => Value::Nil,
    })
}
