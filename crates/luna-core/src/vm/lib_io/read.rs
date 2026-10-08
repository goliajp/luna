//! Read formats and the readers for lines, counts and the whole file.

use super::*;
mod lines_chunks;
pub(crate) use lines_chunks::*;

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
            // `luaL_checkstring`'s type error: a `__name` it found, then
            // the message, pushed
            let tn = argcheck::typename_of(vm, fmt);
            let named = !vm.get_mm(fmt, crate::vm::exec::Mm::Name).is_nil();
            vm.native_push(1 + u32::from(named));
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
    reset_errno(vm);
    if u.crt.is_some() {
        crt::begin_read(u);
        let r = g_read_formats(vm, u, fmts, argno0)?;
        return Ok(if crt::ferror(u) {
            ReadOut::Error(crt::error())
        } else {
            r
        });
    }
    // glibc's stdio flushes between writing and reading the same stream
    if let Err(e) = drain_write_buf(u) {
        return Ok(ReadOut::Error(e));
    }
    g_read_formats(vm, u, fmts, argno0)
}

fn g_read_formats(
    vm: &mut Vm,
    u: Gc<Userdata>,
    fmts: &[Value],
    argno0: u32,
) -> Result<ReadOut, LuaError> {
    if fmts.is_empty() {
        return Ok(match read_line(vm, u, false) {
            Ok(v) => ReadOut::Values(vec![v]),
            Err(e) => ReadOut::Error(e),
        });
    }
    let mut out = Vec::with_capacity(fmts.len());
    for (i, &f) in fmts.iter().enumerate() {
        let fmt = match parse_format(vm, f, argno0 + i as u32) {
            Ok(f) => f,
            Err(e) => {
                // over the values read before it
                vm.native_push(out.len() as u32);
                return Err(e);
            }
        };
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
    if u.crt.is_some() {
        let got = if vm.version() == LuaVersion::Lua51 {
            read_chars_51(u, size.min(usize::MAX as u64) as usize)
        } else {
            crt::fread(u, size as usize)
        };
        return Ok(if got.is_empty() {
            Ok(Value::Nil)
        } else {
            read_str(vm, &got)
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
