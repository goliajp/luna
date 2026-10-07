//! Environment, files and processes: `os.getenv`, `os.remove`, `os.rename`, `os.tmpname`, `os.execute`, `os.exit` and `os.setlocale`.

use super::*;

pub(super) fn os_getenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let v = match std::env::var_os(lib_io::os_path(&name)) {
        Some(val) => Value::Str(vm.heap.intern(&os_bytes(&val))),
        None => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[v]))
}

use lib_io::os_bytes;

/// C `remove`: `rmdir` for a directory, `unlink` otherwise.
#[cfg(not(windows))]
fn remove_path(name: &[u8]) -> std::io::Result<()> {
    let p = lib_io::os_path(name);
    if std::fs::symlink_metadata(&p)?.is_dir() {
        std::fs::remove_dir(p)
    } else {
        std::fs::remove_file(p)
    }
}

/// The Universal CRT's `remove`, which removes files only.
#[cfg(windows)]
fn remove_path(name: &[u8]) -> std::io::Result<()> {
    lib_io::winfs::remove(name)
}

/// C `rename`; the Universal CRT's does not replace an existing file.
fn rename_path(from: &[u8], to: &[u8]) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        lib_io::winfs::rename(from, to)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(lib_io::os_path(from), lib_io::os_path(to))
    }
}

pub(super) fn os_remove(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    lib_io::reset_errno(vm);
    Ok(match remove_path(&name) {
        Ok(()) => vm.nat_return(fs, &[Value::Bool(true)]),
        Err(e) => lib_io::file_fail(vm, fs, Some(&name), &e),
    })
}

pub(super) fn os_rename(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let from = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
    let to = argcheck::check_string(vm, a, 1)?.as_bytes().to_vec();
    lib_io::reset_errno(vm);
    Ok(match rename_path(&from, &to) {
        Ok(()) => vm.nat_return(fs, &[Value::Bool(true)]),
        // 5.1 names the source file in the message; 5.2+ name nothing
        Err(e) => {
            let fname = (vm.version() == LuaVersion::Lua51).then_some(from.as_slice());
            lib_io::file_fail(vm, fs, fname, &e)
        }
    })
}

/// `os.tmpname` on Windows: the Universal CRT's `tmpnam`, which creates
/// nothing. Its names are `s<process id>.<n>` in the temporary directory,
/// both numbers in base 36, `n` counting up through the process; it skips
/// names that exist, and the check of the free one leaves ENOENT.
#[cfg(windows)]
pub(super) fn os_tmpname(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    static NEXT: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    fn base36(mut n: u64) -> String {
        let mut d = Vec::new();
        loop {
            d.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(n % 36) as usize]);
            n /= 36;
            if n == 0 {
                break;
            }
        }
        d.reverse();
        String::from_utf8(d).expect("ASCII digits")
    }
    let mut dir = std::env::temp_dir().display().to_string();
    if !dir.ends_with('\\') {
        dir.push('\\');
    }
    // `L_tmpnam` less the room the name needs
    if dir.len() > 260 - 22 {
        return Err(raise_str(vm, "unable to generate a unique filename"));
    }
    let mut next = NEXT.lock().unwrap_or_else(|e| e.into_inner());
    let pid = base36(u64::from(std::process::id()));
    let name = loop {
        let name = format!("{dir}s{pid}.{}", base36(*next));
        *next += 1;
        if std::fs::metadata(&name).is_err() {
            break name;
        }
    };
    crate::cerrno::set(2);
    let s = Value::Str(vm.heap.intern(name.as_bytes()));
    Ok(vm.nat_return(fs, &[s]))
}

/// `os.tmpname`: POSIX builds use `mkstemp("/tmp/lua_XXXXXX")`, which
/// creates the file it names.
#[cfg(not(windows))]
pub(super) fn os_tmpname(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    use std::hash::{BuildHasher, Hasher};
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let dir = if cfg!(unix) {
        std::path::PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    // mkstemp's own retry budget
    for _ in 0..100 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .expect("the clock reads after 1970"),
        );
        let mut bits = h.finish();
        let name: String = (0..6)
            .map(|_| {
                let c = CHARS[(bits % CHARS.len() as u64) as usize] as char;
                bits /= CHARS.len() as u64;
                c
            })
            .collect();
        let path = dir.join(format!("lua_{name}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => {
                let s = Value::Str(vm.heap.intern(os_bytes(path.as_os_str()).as_slice()));
                return Ok(vm.nat_return(fs, &[s]));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => break,
        }
    }
    Err(raise_str(vm, "unable to generate a unique filename"))
}

/// `os.execute([command])`: `system(3)`.
#[cfg(any(unix, windows))]
pub(super) fn os_execute(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let cmd = argcheck::opt_string(vm, Args::new(fs, nargs), 0)?.map(|s| s.as_bytes().to_vec());
    let v = vm.version();
    lib_io::reset_errno(vm);
    let Some(cmd) = cmd else {
        // system(NULL): whether a shell exists
        return Ok(if v == LuaVersion::Lua51 {
            vm.nat_return(fs, &[Value::Int(1)])
        } else {
            vm.nat_return(fs, &[Value::Bool(true)])
        });
    };
    let status = lib_io::shell_command(&cmd).status();
    if v == LuaVersion::Lua51 {
        // 5.1 returns system()'s raw wait status
        let raw = match status {
            Ok(s) => raw_wait_status(&s),
            Err(_) => -1,
        };
        return Ok(vm.nat_return(fs, &[Value::Int(raw as i64)]));
    }
    Ok(lib_io::exec_result(vm, fs, status))
}

#[cfg(unix)]
fn raw_wait_status(s: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.into_raw()
}

#[cfg(windows)]
fn raw_wait_status(s: &std::process::ExitStatus) -> i32 {
    s.code().expect("a Windows process always has an exit code")
}

/// Targets without processes (`wasm32-wasip1`): no shell; a command fails
/// the way `system` does when it cannot run one.
#[cfg(not(any(unix, windows)))]
pub(super) fn os_execute(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let cmd = argcheck::opt_string(vm, Args::new(fs, nargs), 0)?;
    let v = vm.version();
    if cmd.is_none() {
        return Ok(if v == LuaVersion::Lua51 {
            vm.nat_return(fs, &[Value::Int(0)])
        } else {
            vm.nat_return(fs, &[Value::Bool(false)])
        });
    }
    if v == LuaVersion::Lua51 {
        return Ok(vm.nat_return(fs, &[Value::Int(-1)]));
    }
    let kind = Value::Str(vm.heap.intern(b"exit"));
    Ok(vm.nat_return(fs, &[Value::Nil, kind, Value::Int(-1)]))
}

/// `os.exit([code [, close]])`. Like C `exit`, every stream's pending
/// output is written first; with `close` (5.2+) the state is closed too,
/// running its finalizers.
pub(super) fn os_exit(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let code = match a.get(vm, 0) {
        Value::Bool(b) if vm.version() >= LuaVersion::Lua52 && !a.is_none(0) => {
            if b {
                0
            } else {
                1
            }
        }
        _ => argcheck::opt_integer(vm, a, 0, 0)? as i32,
    };
    if vm.version() >= LuaVersion::Lua52 && a.get(vm, 1).truthy() {
        vm.close_state();
    }
    lib_io::flush_all(vm);
    std::process::exit(code);
}

/// Locale categories `os.setlocale` names, and the one more that "all"
/// covers (LC_MESSAGES), in the order a mixed query lists them.
pub(super) const LC_COUNT: usize = 6;
const LC_NAMES: [&str; 6] = ["all", "collate", "ctype", "monetary", "numeric", "time"];

/// `os.setlocale([locale [, category]])`. luna has the C locale only, which
/// is also known as "POSIX"; "" selects the one the environment names.
pub(super) fn os_setlocale(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let name = argcheck::opt_string(vm, a, 0)?.map(|s| lib_io::c_str(s.as_bytes()).to_vec());
    let cat = argcheck::check_option(vm, a, 1, Some("all"), &LC_NAMES)?;
    // upvalues 0..6: collate, ctype, monetary, numeric, time, messages
    let slots: Vec<usize> = if cat == 0 {
        (0..LC_COUNT).collect()
    } else {
        vec![cat - 1]
    };
    if let Some(name) = name {
        let resolved = if name.is_empty() {
            env_locale(LC_NAMES[cat])
        } else {
            name
        };
        if resolved != b"C" && resolved != b"POSIX" {
            return Ok(vm.nat_return(fs, &[Value::Nil]));
        }
        let v = Value::Str(vm.heap.intern(&resolved));
        for &i in &slots {
            vm.nat_set_upval(fs, i, v);
        }
    }
    let names: Vec<Vec<u8>> = slots
        .iter()
        .map(|&i| match vm.nat_upval(fs, i) {
            Value::Str(s) => s.as_bytes().to_vec(),
            _ => unreachable!("only os.setlocale writes its upvalues, always strings"),
        })
        .collect();
    // a mixed "all" reads as the categories joined by '/'
    let text = if names.iter().all(|n| *n == names[0]) {
        names[0].clone()
    } else {
        names.join(&b'/')
    };
    let r = Value::Str(vm.heap.intern(&text));
    Ok(vm.nat_return(fs, &[r]))
}

/// The locale `setlocale(cat, "")` picks: LC_ALL, then the category's own
/// variable, then LANG, else "C".
fn env_locale(cat: &str) -> Vec<u8> {
    let own = format!("LC_{}", cat.to_ascii_uppercase());
    for var in ["LC_ALL", own.as_str(), "LANG"] {
        if let Some(v) = std::env::var_os(var)
            && !v.is_empty()
        {
            return os_bytes(&v);
        }
    }
    b"C".to_vec()
}
