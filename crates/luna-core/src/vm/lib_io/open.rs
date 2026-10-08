//! Opening files and pipes, and the default input/output streams.

use super::*;
mod popen;
pub(crate) use popen::*;

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
        // the new handle is pushed before the mode is checked
        vm.native_push(1);
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
    // `getiofile` pushes the stream from the registry
    vm.native_push(1);
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
            // over the new handle; 5.1 pushes the message for the argument
            // error as well
            let v51 = vm.version() == LuaVersion::Lua51;
            vm.native_push(1 + u32::from(v51));
            Err(if v51 {
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
