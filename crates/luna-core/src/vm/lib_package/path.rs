//! Path handling: `package.path` / `cpath` setup and `package.searchpath`.

use super::*;

/// 5.4's `setpath` on an environment's path: the default in place of its
/// first ";;", a separator left dangling dropped.
pub(super) fn splice_default(path: &[u8], dft: &[u8]) -> Vec<u8> {
    let Some(at) = path.windows(2).position(|w| w == b";;") else {
        return path.to_vec();
    };
    let mut out = Vec::new();
    if at > 0 {
        out.extend_from_slice(&path[..at]);
        out.push(b';');
    }
    out.extend_from_slice(dft);
    if at + 2 < path.len() {
        out.push(b';');
        out.extend_from_slice(&path[at + 2..]);
    }
    out
}

pub(super) use crate::vm::lib_io::os_bytes;

/// PUC's `LUA_CPATH_DEFAULT` for the target, its install prefix included,
/// as `setpath` leaves it (on Windows `!` stands for the executable's
/// directory). luna finds C libraries there but loads none.
pub(super) fn cpath_default(v: LuaVersion) -> Vec<u8> {
    let vdir = match v {
        LuaVersion::Lua51 => "5.1",
        LuaVersion::Lua52 => "5.2",
        LuaVersion::Lua53 => "5.3",
        LuaVersion::Lua54 => "5.4",
        _ => "5.5",
    };
    if cfg!(windows) {
        let d = if v == LuaVersion::Lua51 {
            ".\\?.dll;!\\?.dll;!\\loadall.dll".to_string()
        } else if v == LuaVersion::Lua52 {
            "!\\?.dll;!\\loadall.dll;.\\?.dll".to_string()
        } else {
            format!("!\\?.dll;!\\..\\lib\\lua\\{vdir}\\?.dll;!\\loadall.dll;.\\?.dll")
        };
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| os_bytes(d.as_os_str())));
        return match exe_dir {
            Some(dir) => replace(d.as_bytes(), b"!", &dir),
            None => d.into_bytes(),
        };
    }
    let cdir = format!("/usr/local/lib/lua/{vdir}/");
    if v == LuaVersion::Lua51 {
        format!("./?.so;{cdir}?.so;{cdir}loadall.so").into_bytes()
    } else {
        format!("{cdir}?.so;{cdir}loadall.so;./?.so").into_bytes()
    }
}

/// `luaL_gsub`: replace every `from` in `src` with `to`.
pub(super) fn replace(src: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(src[i]);
            i += 1;
        }
    }
    out
}

/// `readable`: whether `fopen(name, "r")` succeeds.
pub(super) fn readable(name: &[u8]) -> bool {
    match crate::vm::lib_io::fopen::os_open(name, &crate::vm::lib_io::fopen::Spec::READ) {
        Ok(_) => true,
        Err(e) => {
            // the failed `fopen` leaves its errno
            crate::vm::lib_io::note_failure(&e);
            false
        }
    }
}

/// `searchpath`: the first readable file among `path`'s templates with
/// `name` (its `sep`s turned into `dirsep`) in place of each '?', or the
/// "no file" message listing every candidate. ≤5.3 skip empty templates
/// and start each entry with "\n\t"; 5.4 keeps empty ones and joins them.
pub(super) fn search_path(
    v: LuaVersion,
    name: &[u8],
    path: &[u8],
    sep: &[u8],
    dirsep: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let name = if sep.is_empty() {
        name.to_vec()
    } else {
        replace(name, sep, dirsep)
    };
    let mut err = Vec::new();
    if v <= LuaVersion::Lua53 {
        for tpl in path.split(|&b| b == b';').filter(|t| !t.is_empty()) {
            let file = replace(tpl, b"?", &name);
            if readable(&file) {
                return Ok(file);
            }
            err.extend_from_slice(b"\n\tno file '");
            err.extend_from_slice(&file);
            err.push(b'\'');
        }
        return Err(err);
    }
    let expanded = replace(path, b"?", &name);
    for file in expanded.split(|&b| b == b';') {
        if readable(file) {
            return Ok(file.to_vec());
        }
    }
    err.extend_from_slice(b"no file '");
    err.extend_from_slice(&replace(&expanded, b";", b"'\n\tno file '"));
    err.push(b'\'');
    Err(err)
}

pub(super) fn ll_searchpath(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let name = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
    let path = argcheck::check_string(vm, a, 1)?.as_bytes().to_vec();
    let sep = match argcheck::opt_string(vm, a, 2)? {
        Some(s) => s.as_bytes().to_vec(),
        None => b".".to_vec(),
    };
    let dirsep = match argcheck::opt_string(vm, a, 3)? {
        Some(s) => s.as_bytes().to_vec(),
        None => b"/".to_vec(),
    };
    let r = search_path(
        vm.version(),
        c_str(&name),
        c_str(&path),
        c_str(&sep),
        c_str(&dirsep),
    );
    match r {
        Ok(file) => {
            let f = vm.str_value(&file)?;
            vm.nat_ret(fs, &[f])
        }
        Err(msg) => {
            let m = vm.str_value(&msg)?;
            vm.nat_ret(fs, &[Value::Nil, m])
        }
    }
}

/// `findfile`: search `package[pname]` (which must be a string) for `name`.
pub(super) fn find_file(
    vm: &mut Vm,
    pkg: Gc<Table>,
    name: &[u8],
    pname: &str,
) -> Result<Result<Vec<u8>, Vec<u8>>, LuaError> {
    let path = vm.native_getfield(Value::Table(pkg), pname.as_bytes())?;
    let Some(path) = argcheck::to_str_bytes(vm, path) else {
        return Err(crate::vm::builtins::raise_fmt(
            vm,
            1,
            &format!("'package.{pname}' must be a string"),
        ));
    };
    // 5.1 turns every '.' of the name into the directory separator itself
    Ok(search_path(
        vm.version(),
        c_str(name),
        c_str(&path),
        b".",
        b"/",
    ))
}
