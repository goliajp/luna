//! Opening files and pipes, and the default input/output streams.

use super::*;

/// `l_checkmode`: 5.2 accepts `[rwa]%+?b?`, 5.3+ `[rwa]%+?b*`; 5.1 checks
/// nothing and lets `fopen` decide.
fn mode_ok(v: LuaVersion, mode: &[u8]) -> bool {
    let Some((&first, rest)) = mode.split_first() else {
        return false;
    };
    if !b"rwa".contains(&first) {
        return false;
    }
    let rest = rest.strip_prefix(b"+").unwrap_or(rest);
    match v {
        LuaVersion::Lua52 => rest.is_empty() || rest == b"b",
        _ => rest.iter().all(|&c| c == b'b'),
    }
}

/// The handle for a file `fopen` opened, with the C library's `FILE` when
/// the Vm opens files as PUC built with MSVC does.
fn opened(vm: &mut Vm, o: Opened, pipe: bool) -> Gc<Userdata> {
    let u = new_file(vm, FileHandle::File(o.file), o.spec.writable());
    if vm.crt_text {
        let mut f = msvc::CrtFile::open(&o.spec, pipe);
        f.io.mode = o.mode;
        f.io.unicode = o.spec.ccs == Some(fopen::Ccs::Unicode);
        crt::attach(u, f);
    }
    u
}

/// A file `fopen` opened: the file, the mode as the library took it, and
/// the text mode of the handle.
pub(crate) struct Opened {
    pub(crate) file: std::fs::File,
    pub(crate) spec: fopen::Spec,
    pub(crate) mode: msvc::TextMode,
}

/// `fopen(name, mode)`. A Vm that opens files as PUC built with MSVC does
/// reads the mode as that C library does, and ends the process where it
/// would.
pub(crate) fn open_file(crt: bool, name: &[u8], mode: &[u8]) -> std::io::Result<Opened> {
    let mode = c_str(mode);
    let spec = if crt {
        if mode.is_empty() {
            crt::invalid_parameter();
        }
        // checked ahead of the mode, without ending the process
        if c_str(name).is_empty() {
            return Err(posix_error(EINVAL));
        }
        fopen::ucrt_mode(mode).unwrap_or_else(|| crt::invalid_parameter())
    } else {
        fopen::libc_mode(mode).ok_or_else(|| posix_error(EINVAL))?
    };
    let mut file = fopen::os_open(name, &spec)?;
    let text = crt && !spec.binary;
    if text && spec.update && file.metadata()?.is_file() {
        // `truncate_ctrl_z_if_present`: its seek to the last byte of an
        // empty file fails, which leaves EINVAL in `errno`
        if file.metadata()?.len() == 0 {
            crate::cerrno::set(EINVAL);
        } else {
            // through a handle of its own: an append handle may not shorten
            let mut g = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(os_path(name))?;
            msvc::drop_final_ctrl_z(&mut g)?;
        }
    }
    let mode = if text {
        fopen::text_mode(&mut file, &spec)?
    } else {
        msvc::TextMode::Ansi
    };
    Ok(Opened { file, spec, mode })
}

pub(super) fn io_open(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let name = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
    let mode = match argcheck::opt_string(vm, a, 1)? {
        Some(m) => m.as_bytes().to_vec(),
        None => b"r".to_vec(),
    };
    if vm.version() >= LuaVersion::Lua52 && !mode_ok(vm.version(), &mode) {
        return Err(arg_error(vm, 2, "invalid mode"));
    }
    reset_errno(vm);
    match open_file(vm.crt_text, &name, &mode) {
        Ok(o) => {
            let u = opened(vm, o, false);
            Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
        }
        Err(e) => Ok(file_fail(vm, fs, Some(&name), &e)),
    }
}

pub(super) fn io_tmpfile(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    reset_errno(vm);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("lua_tmp_{}_{n}", std::process::id()));
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => return Ok(file_fail(vm, fs, None, &e)),
    };
    // tmpfile(3) leaves no name behind; the open handle keeps the file.
    if let Err(e) = std::fs::remove_file(&path) {
        return Ok(file_fail(vm, fs, None, &e));
    }
    // tmpfile(3) opens "w+bD"
    let spec = fopen::libc_mode(b"w+b").expect("a valid mode");
    let o = Opened {
        file,
        spec,
        mode: msvc::TextMode::Ansi,
    };
    let u = opened(vm, o, false);
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

/// `io.popen(prog [, mode])`: a `/bin/sh -c prog` child with its stdout
/// (`"r"`) or stdin (`"w"`) as the stream.
#[cfg(any(unix, windows))]
pub(super) fn io_popen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let prog = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
    let mode = match argcheck::opt_string(vm, a, 1)? {
        Some(m) => m.as_bytes().to_vec(),
        None => b"r".to_vec(),
    };
    // 5.3+ check the mode (`l_checkmodep`); before that popen(3) did, and
    // refuses anything but r/w with EINVAL. ("r+"/"w+", a two-way stream on
    // BSD popen, is not provided.)
    let read = match c_str(&mode) {
        b"r" => true,
        b"w" => false,
        _ if vm.version() >= LuaVersion::Lua53 => return Err(arg_error(vm, 2, "invalid mode")),
        _ => {
            let e = posix_error(EINVAL);
            return Ok(file_fail(vm, fs, Some(&prog), &e));
        }
    };
    reset_errno(vm);
    // `l_popen` flushes every output stream first (`fflush(NULL)`), so the
    // child sees what the parent wrote before it.
    flush_all(vm);
    let mut cmd = shell_command(&prog);
    if read {
        cmd.stdout(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Ok(file_fail(vm, fs, Some(&prog), &e)),
    };
    let file = if read {
        pipe_file(child.stdout.take().expect("stdout was piped"))
    } else {
        pipe_file(child.stdin.take().expect("stdin was piped"))
    };
    let spec = fopen::libc_mode(if read { b"r" } else { b"w" }).expect("a valid mode");
    let o = Opened {
        file,
        spec,
        mode: msvc::TextMode::Ansi,
    };
    let u = opened(vm, o, true);
    // SAFETY: `u` was created by `opened` just above and is held only by this local; the borrow covers one field store
    unsafe { u.as_mut() }.popen_child = Some(child);
    Ok(vm.nat_return(fs, &[Value::Userdata(u)]))
}

/// A child's pipe end as a plain file, so reads and writes share one path.
#[cfg(unix)]
fn pipe_file(p: impl Into<std::os::fd::OwnedFd>) -> std::fs::File {
    std::fs::File::from(p.into())
}

#[cfg(windows)]
fn pipe_file(p: impl Into<std::os::windows::io::OwnedHandle>) -> std::fs::File {
    std::fs::File::from(p.into())
}

/// Targets without processes (`wasm32-wasip1`): the ISO C `l_popen`, which
/// raises "'popen' not supported" after the argument checks.
#[cfg(not(any(unix, windows)))]
pub(super) fn io_popen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    argcheck::check_string(vm, a, 0)?;
    argcheck::opt_string(vm, a, 1)?;
    Err(raise_str(vm, "'popen' not supported"))
}

/// The shell `system(3)` and `popen(3)` run a command through.
#[cfg(any(unix, windows))]
pub(crate) fn shell_command(cmd: &[u8]) -> std::process::Command {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut c = std::process::Command::new("/bin/sh");
        c.arg("-c").arg(std::ffi::OsStr::from_bytes(c_str(cmd)));
        c
    }
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C")
            .arg(String::from_utf8_lossy(c_str(cmd)).into_owned());
        c
    }
}

// ---- default streams ----

#[derive(Clone, Copy)]
pub(super) enum Io {
    Input,
    Output,
}

pub(super) fn default_file(vm: &Vm, which: Io) -> Gc<Userdata> {
    match which {
        Io::Input => vm.io_input,
        Io::Output => vm.io_output,
    }
    .expect("default streams are set when io opens")
}

/// `getiofile`: the default stream, which must still be open.
pub(super) fn get_io_file(vm: &mut Vm, which: Io) -> Result<Gc<Userdata>, LuaError> {
    let u = default_file(vm, which);
    if u.file().is_closed() {
        let what = match which {
            Io::Input => "input",
            Io::Output => "output",
        };
        let adj = if vm.version() >= LuaVersion::Lua54 {
            "default"
        } else {
            "standard"
        };
        return Err(raise_str(vm, &format!("{adj} {what} file is closed")));
    }
    Ok(u)
}

/// `g_iofile`: set the default stream from a file name or a handle, then
/// return it.
fn g_iofile(vm: &mut Vm, fs: u32, nargs: u32, which: Io) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if !a.is_none_or_nil(vm, 0) {
        let v = a.get(vm, 0);
        let u = match argcheck::to_str_bytes(vm, v) {
            Some(name) => {
                let mode: &[u8] = match which {
                    Io::Input => b"r",
                    Io::Output => b"w",
                };
                open_checked(vm, &name, mode)?
            }
            None => check_open(vm, a, 0)?,
        };
        match which {
            Io::Input => vm.io_input = Some(u),
            Io::Output => vm.io_output = Some(u),
        }
    }
    let cur = default_file(vm, which);
    Ok(vm.nat_return(fs, &[Value::Userdata(cur)]))
}

/// `opencheck` (5.2+) / 5.1's `fileerror`: open or raise.
pub(super) fn open_checked(
    vm: &mut Vm,
    name: &[u8],
    mode: &[u8],
) -> Result<Gc<Userdata>, LuaError> {
    match open_file(vm.crt_text, name, mode) {
        Ok(o) => Ok(opened(vm, o, false)),
        Err(e) => {
            note_failure(&e);
            let n = String::from_utf8_lossy(c_str(name)).into_owned();
            let err = strerror(&e);
            Err(if vm.version() == LuaVersion::Lua51 {
                arg_error(vm, 1, &format!("{n}: {err}"))
            } else {
                raise_str(vm, &format!("cannot open file '{n}' ({err})"))
            })
        }
    }
}

pub(super) fn io_input(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    g_iofile(vm, fs, nargs, Io::Input)
}

pub(super) fn io_output(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    g_iofile(vm, fs, nargs, Io::Output)
}
