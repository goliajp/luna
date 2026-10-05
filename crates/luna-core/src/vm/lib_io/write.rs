//! Writing, flushing, seeking and buffering mode.

use super::*;

/// How the dialect writes a number: ≤5.2 `%.14g`; 5.3/5.4 `%lld` for an
/// integer and `%.14g` for a float (so no ".0"); 5.5 converts as tostring.
fn number_text(vm: &Vm, n: Num) -> Vec<u8> {
    let fmt = match vm.version() {
        LuaVersion::Lua55 => vm.float_fmt(),
        _ => FloatFmt::Legacy14,
    };
    numeric::num_to_string_for(n, fmt).into_bytes()
}

/// `g_write`: write `vals` in order and give the dialect's result.
fn g_write(vm: &mut Vm, fs: u32, u: Gc<Userdata>, args: Args, first: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let mut total: i64 = 0;
    let mut failure: Option<std::io::Error> = None;
    for i in first..args.n {
        let bytes = match args.get(vm, i) {
            Value::Int(x) => number_text(vm, Num::Int(x)),
            Value::Float(f) => number_text(vm, Num::Float(f)),
            _ => argcheck::check_string(vm, args, i)?.as_bytes().to_vec(),
        };
        // ≤5.4 stop writing after a failure but still check the remaining
        // arguments; 5.5 returns at the first failure
        if failure.is_some() {
            continue;
        }
        match put_bytes(u, &bytes) {
            Ok(()) => total += bytes.len() as i64,
            Err(e) if v >= LuaVersion::Lua55 => {
                let mut vals = file_fail_values(vm, None, &e).to_vec();
                vals.push(Value::Int(total));
                return Ok(vm.nat_return(fs, &vals));
            }
            Err(e) => failure = Some(e),
        }
    }
    Ok(match failure {
        Some(e) => file_fail(vm, fs, None, &e),
        // 5.1 reports success as true; 5.2+ return the file
        None if v == LuaVersion::Lua51 => file_ok(vm, fs),
        None => vm.nat_return(fs, &[Value::Userdata(u)]),
    })
}

pub(super) fn io_write(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = get_io_file(vm, Io::Output)?;
    g_write(vm, fs, u, Args::new(fs, nargs), 0)
}

pub(super) fn f_write(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let u = check_open(vm, a, 0)?;
    g_write(vm, fs, u, a, 1)
}

pub(super) fn io_flush(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let u = get_io_file(vm, Io::Output)?;
    Ok(match flush_stream(u) {
        Ok(()) => file_ok(vm, fs),
        Err(e) => file_fail(vm, fs, None, &e),
    })
}

pub(super) fn f_flush(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let u = check_open(vm, Args::new(fs, nargs), 0)?;
    Ok(match flush_stream(u) {
        Ok(()) => file_ok(vm, fs),
        Err(e) => file_fail(vm, fs, None, &e),
    })
}

// ---- seek / setvbuf ----

pub(super) fn f_seek(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let u = check_open(vm, a, 0)?;
    let op = argcheck::check_option(vm, a, 1, Some("cur"), &["set", "cur", "end"])?;
    let offset = if vm.version() == LuaVersion::Lua52 {
        // 5.2 reads a float and requires it to survive the cast to off_t
        let p3 = argcheck::opt_number(vm, a, 2, 0.0)?;
        let off = p3 as i64;
        if off as f64 != p3 {
            return Err(arg_error(vm, 3, "not an integer in proper range"));
        }
        off
    } else {
        argcheck::opt_integer(vm, a, 2, 0)?
    };
    match seek_stream(u, op, offset) {
        // ≤5.2 has one number type: `lua_pushnumber(ftell(f))`
        Ok(pos) if vm.version() <= LuaVersion::Lua52 => {
            Ok(vm.nat_return(fs, &[Value::Float(pos as f64)]))
        }
        Ok(pos) => Ok(vm.nat_return(fs, &[Value::Int(pos as i64)])),
        Err(e) => Ok(file_fail(vm, fs, None, &e)),
    }
}

/// `fseek` + `ftell`: flush pending output, give back read-ahead, move.
fn seek_stream(u: Gc<Userdata>, op: usize, offset: i64) -> std::io::Result<u64> {
    if u.text.is_some() && matches!(u.file(), FileHandle::File(_)) {
        return text_mode::fseek(u, op, offset);
    }
    drain_write_buf(u)?;
    if matches!(u.file(), FileHandle::Stdout) {
        // liolib always calls `fseek`, which writes out what stdout
        // buffers, even where the move then fails (a pipe)
        crate::stdio::flush_stdout()?;
    }
    let ahead = read_ahead(u);
    // SAFETY: `u` is a file handle the caller holds; `drain_write_buf` and `read_ahead` have returned, and `m` is the only reference into it until return
    let m = unsafe { u.as_mut() };
    let from = match op {
        0 if offset < 0 => return Err(posix_error(EINVAL)),
        0 => SeekFrom::Start(offset as u64),
        1 => match offset.checked_sub(ahead) {
            Some(off) => SeekFrom::Current(off),
            None => return Err(posix_error(EINVAL)),
        },
        _ => SeekFrom::End(offset),
    };
    let pos = match m.file_mut() {
        FileHandle::File(f) => f.seek(from)?,
        std_stream => seek_std(std_stream, from)?,
    };
    m.read_buf = Vec::new();
    m.read_pos = 0;
    Ok(pos)
}

/// Seek a standard stream through a duplicate of its descriptor, which
/// shares the offset (and fails with ESPIPE on a terminal or pipe).
#[cfg(unix)]
fn seek_std(fh: &FileHandle, from: SeekFrom) -> std::io::Result<u64> {
    use std::os::fd::AsFd;
    let fd = match fh {
        FileHandle::Stdin => std::io::stdin().as_fd().try_clone_to_owned()?,
        FileHandle::Stdout => std::io::stdout().as_fd().try_clone_to_owned()?,
        FileHandle::Stderr => std::io::stderr().as_fd().try_clone_to_owned()?,
        FileHandle::File(_) | FileHandle::Closed => unreachable!("only standard streams"),
    };
    std::fs::File::from(fd).seek(from)
}

#[cfg(not(unix))]
fn seek_std(_fh: &FileHandle, _from: SeekFrom) -> std::io::Result<u64> {
    Err(posix_error(ESPIPE))
}

pub(super) fn f_setvbuf(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let u = check_open(vm, a, 0)?;
    let op = argcheck::check_option(vm, a, 1, None, &["no", "full", "line"])?;
    argcheck::opt_integer(vm, a, 2, LUAL_BUFFERSIZE)?;
    let mode = [BUF_NO, BUF_FULL, BUF_LINE][op];
    // SAFETY: `u` came from `check_open` on a native argument, so the stack keeps it; the borrow covers one field store
    unsafe { u.as_mut() }.buf_mode = mode;
    if matches!(u.file(), FileHandle::Stdout) {
        crate::stdio::setvbuf_stdout(mode);
    }
    if mode == BUF_NO
        && let Err(e) = drain_write_buf(u)
    {
        return Ok(file_fail(vm, fs, None, &e));
    }
    Ok(file_ok(vm, fs))
}
