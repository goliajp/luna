//! Read formats and the readers for lines, counts and the whole file.

use super::*;
use crate::runtime::mem::LVec;
mod buffered;
mod lines_chunks;
pub(crate) use buffered::*;
pub(crate) use lines_chunks::*;

/// Outcome of `g_read`: the values (the last one nil when a format failed),
/// or the I/O error that ends a read (`ferror`).
pub(super) enum ReadOut {
    Values(Vec<Value>),
    Error(std::io::Error),
}

/// Why a read format stopped: an I/O error (`ferror`, a soft failure), or a
/// Lua error such as a buffer the allocation context refused.
pub(crate) enum RdErr {
    Io(std::io::Error),
    Lua(LuaError),
}

impl From<std::io::Error> for RdErr {
    fn from(e: std::io::Error) -> RdErr {
        RdErr::Io(e)
    }
}

impl From<LuaError> for RdErr {
    fn from(e: LuaError) -> RdErr {
        RdErr::Lua(e)
    }
}

impl From<crate::runtime::mem::Oom> for RdErr {
    fn from(o: crate::runtime::mem::Oom) -> RdErr {
        RdErr::Lua(o.into())
    }
}

pub(crate) type Rd = Result<Value, RdErr>;

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
        #[cfg(target_pointer_width = "32")]
        Value::Int(n) if v <= LuaVersion::Lua52 => {
            return Ok(Fmt::Count(crate::vm::c_abi::number2integer(v, n as f64)));
        }
        Value::Int(n) => return Ok(Fmt::Count(n)),
        Value::Float(f) if v <= LuaVersion::Lua52 => {
            return Ok(Fmt::Count(crate::vm::c_abi::number2integer(v, f)));
        }
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
            vm.metafield_check(fmt, b"__name")?;
            let named = !vm.get_mm(fmt, crate::vm::exec::Mm::Name).is_nil();
            vm.native_push(u32::from(named));
            use crate::runtime::puc_fmt::FmtArg::S;
            let fmt = b"%s expected, got %s";
            return Err(
                match argcheck::pushed_extra(vm, 2, fmt, &[S(b"string"), S(tn.as_bytes())]) {
                    Ok(m) => arg_error(vm, argno, &m),
                    Err(e) => e,
                },
            );
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
/// line. `argno0` numbers the first format in argument errors; `top` is the
/// C function's stack top PUC has when it calls `g_read`.
pub(super) fn g_read(
    vm: &mut Vm,
    u: Gc<Userdata>,
    fmts: &[Value],
    argno0: u32,
    top: u32,
) -> Result<ReadOut, LuaError> {
    reset_errno(vm);
    if u.crt.is_some() {
        crt::begin_read(u);
        let r = g_read_formats(vm, u, fmts, argno0, top)?;
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
    g_read_formats(vm, u, fmts, argno0, top)
}

fn g_read_formats(
    vm: &mut Vm,
    u: Gc<Userdata>,
    fmts: &[Value],
    argno0: u32,
    top: u32,
) -> Result<ReadOut, LuaError> {
    // the values read stay on the C function's stack while the next is
    // read, which may collect
    let base = vm.lib_roots.mark();
    let r = read_formats(vm, u, fmts, argno0, top);
    vm.lib_roots.release(base);
    r
}

fn read_formats(
    vm: &mut Vm,
    u: Gc<Userdata>,
    fmts: &[Value],
    argno0: u32,
    top: u32,
) -> Result<ReadOut, LuaError> {
    if fmts.is_empty() {
        return Ok(match read_line(vm, u, false) {
            Ok(v) => ReadOut::Values(vec![v]),
            Err(RdErr::Io(e)) => ReadOut::Error(e),
            Err(RdErr::Lua(e)) => return Err(e),
        });
    }
    // room for all the results and the auxiliary library's buffer
    let n = fmts.len() as i64 + i64::from(crate::vm::exec::stack_size::MIN_STACK);
    crate::vm::argcheck::check_stack_at(vm, top, n, "too many arguments")?;
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
            Fmt::Count(n) => read_count(vm, u, n),
            Fmt::Number => read_number(vm, u),
            Fmt::Line { keep_nl } => read_line(vm, u, keep_nl),
            Fmt::All => read_all(vm, u),
        };
        match r {
            Ok(v) => {
                let stop = v.is_nil();
                vm.lib_roots.push(v);
                out.push(v);
                if stop {
                    break;
                }
            }
            Err(RdErr::Io(e)) => return Ok(ReadOut::Error(e)),
            Err(RdErr::Lua(e)) => return Err(e),
        }
    }
    Ok(ReadOut::Values(out))
}

fn push_read(vm: &mut Vm, fs: u32, r: ReadOut) -> Result<u32, LuaError> {
    match r {
        ReadOut::Values(vals) => vm.nat_ret(fs, &vals),
        ReadOut::Error(e) => file_fail(vm, fs, None, &e),
    }
}

pub(super) fn io_read(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = get_io_file(vm, Io::Input)?;
    let fmts: Vec<Value> = (0..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    // `getiofile` pushed the file
    let r = g_read(vm, u, &fmts, 1, fs + 1 + nargs + 1)?;
    push_read(vm, fs, r)
}

pub(super) fn f_read(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    let fmts: Vec<Value> = (1..nargs).map(|i| vm.nat_arg(fs, nargs, i)).collect();
    let r = g_read(vm, u, &fmts, 2, fs + 1 + nargs)?;
    push_read(vm, fs, r)
}

/// Sizes no allocator grants; PUC's buffer for them fails before reading.
const UNALLOCATABLE: u64 = 1 << 47;

/// A byte-count format: `0` tests for end of file; `n` reads up to `n`
/// bytes (nil when none were left). A negative count is a huge `size_t`.
fn read_count(vm: &mut Vm, u: Gc<Userdata>, n: i64) -> Rd {
    let size = n as u64;
    if size == 0 {
        return test_eof(vm, u);
    }
    // 5.1 reads in chunks, so any size works; 5.2+ size one buffer for the
    // whole request, which the allocator refuses for absurd sizes
    if size >= UNALLOCATABLE && vm.version() >= LuaVersion::Lua52 {
        // 5.3 has pushed the buffer's box, 5.4+ the buffer's placeholder
        if let Err(e) = vm.native_buffinit(0) {
            return Err(RdErr::Lua(e));
        }
        return Err(RdErr::Lua(match vm.version() {
            LuaVersion::Lua52 if size > u64::MAX - 64 => {
                vm.runerror_in_c("memory allocation error: block too big")
            }
            LuaVersion::Lua53 => {
                vm.native_push(1);
                raise_str(vm, "not enough memory for buffer allocation")
            }
            LuaVersion::Lua55 if size >= i64::MAX as u64 => {
                raise_str(vm, "resulting string too large")
            }
            _ => vm.mem_err(),
        }));
    }
    if u.crt.is_some() {
        let got = if vm.version() == LuaVersion::Lua51 {
            read_chars_51(u, size.min(usize::MAX as u64) as usize)
        } else {
            crt::fread(u, size as usize)
        };
        let chunks = count_chunks(vm, size.min(usize::MAX as u64) as usize, got.len());
        let s = buffered(vm, &got, &chunks, false)?;
        return Ok(if got.is_empty() { Value::Nil } else { s });
    }
    let mut buf = LVec::new(vm.heap.mem());
    while (buf.len() as u64) < size {
        let want = (size - buf.len() as u64) as usize;
        if u.read_pos >= u.read_buf.len() {
            match fill(u) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => return Err(e.into()),
            }
        }
        let take = want.min(u.read_buf.len() - u.read_pos);
        buf.extend_from_slice(&u.read_buf[u.read_pos..u.read_pos + take])?;
        // SAFETY: `u` is a file handle the caller holds; the borrow covers one field store
        unsafe { u.as_mut() }.read_pos += take;
    }
    let chunks = count_chunks(vm, size.min(usize::MAX as u64) as usize, buf.len());
    let s = buffered(vm, &buf, &chunks, false)?;
    Ok(if buf.is_empty() { Value::Nil } else { s })
}

/// `test_eof`: "" if a byte is left, nil at end of file.
fn test_eof(vm: &mut Vm, u: Gc<Userdata>) -> Rd {
    let c = getc(u)?;
    if let Some(c) = c {
        unget(u, &[c]);
    }
    // the empty string is pushed whether a byte is left or not
    let s = vm.api_push_string(b"")?;
    Ok(if c.is_some() { s } else { Value::Nil })
}

fn read_number(vm: &mut Vm, u: Gc<Userdata>) -> Rd {
    if vm.version() <= LuaVersion::Lua52 {
        return Ok(scan_double(u)?);
    }
    let buf = read_numeral(u)?;
    Ok(match numeric::str2num(&buf, true, true) {
        Some(Num::Int(i)) => Value::Int(i),
        Some(Num::Float(f)) => Value::Float(f),
        None => Value::Nil,
    })
}
