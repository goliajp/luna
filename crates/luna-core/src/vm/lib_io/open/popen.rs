//! `io.popen` and the shell it runs commands in.

use super::*;
use crate::runtime::puc::api::PucCollector;

/// `io.popen(prog [, mode])`: a `/bin/sh -c prog` child with its stdout
/// (`"r"`) or stdin (`"w"`) as the stream.
#[cfg(any(unix, windows))]
pub(crate) fn io_popen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
        _ if vm.version() >= LuaVersion::Lua53 => {
            // the new handle is made and pushed before the mode is checked
            return failed_handle(vm, |vm| {
                vm.native_push(1);
                Err(arg_error(vm, 2, "invalid mode"))
            });
        }
        _ => {
            let e = posix_error(EINVAL);
            return failed_handle(vm, |vm| file_fail(vm, fs, Some(&prog), &e));
        }
    };
    reset_errno(vm);
    // `l_popen` flushes every output stream first (`fflush(NULL)`), so the
    // child sees what the parent wrote before it.
    flush_all(vm);
    let (child, file) = match spawn_piped(&prog, read) {
        Ok(c) => c,
        Err(e) => return failed_handle(vm, |vm| file_fail(vm, fs, Some(&prog), &e)),
    };
    let spec = fopen::libc_mode(if read { b"r" } else { b"w" }).expect("a valid mode");
    let o = Opened {
        file,
        spec,
        mode: msvc::TextMode::Ansi,
    };
    let u = opened(vm, o, true)?;
    if vm.version() == crate::version::LuaVersion::Lua51 {
        vm.heap
            .set_env51(u.hdr(), crate::runtime::puc::records::env51::ENV_IO_POPEN);
    }
    // SAFETY: `u` was created by `opened` just above and is held only by this local; the borrow covers one field store
    unsafe { u.as_mut() }.popen_child = Some(child);
    vm.nat_ret1(fs, Value::Userdata(u))
}

/// `prog` run by the shell, its stdout (`read`) or stdin piped: the child
/// and the pipe's end.
#[cfg(any(unix, windows))]
fn spawn_piped(prog: &[u8], read: bool) -> std::io::Result<(std::process::Child, std::fs::File)> {
    let mut cmd = shell_command(prog);
    if read {
        cmd.stdout(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::piped());
    }
    let mut child = cmd.spawn()?;
    let file = if read {
        pipe_file(child.stdout.take().expect("stdout was piped"))
    } else {
        pipe_file(child.stdin.take().expect("stdin was piped"))
    };
    Ok((child, file))
}

/// A child's pipe end as a plain file, so reads and writes share one path.
#[cfg(unix)]
pub(crate) fn pipe_file(p: impl Into<std::os::fd::OwnedFd>) -> std::fs::File {
    std::fs::File::from(p.into())
}

#[cfg(windows)]
pub(crate) fn pipe_file(p: impl Into<std::os::windows::io::OwnedHandle>) -> std::fs::File {
    std::fs::File::from(p.into())
}

/// Targets without processes (`wasm32-wasip1`): the ISO C `l_popen`, which
/// raises "'popen' not supported" after the argument checks.
#[cfg(not(any(unix, windows)))]
pub(crate) fn io_popen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
        // `system` hands the command over through the ANSI code page
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(os_path(cmd));
        c
    }
}

// ---- default streams ----
