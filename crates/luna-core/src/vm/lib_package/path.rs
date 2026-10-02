//! Path handling: `package.path` / `cpath` setup and `package.searchpath`.

use super::*;

/// `setpath`: unless `noenv`, the environment's path (5.2+ try `NAME_5_x`
/// first) with ";;" replaced by the default, else the default. 5.1–5.3
/// replace every ";;"; 5.4 replaces the first and drops a separator left
/// dangling.
pub(super) fn env_path(v: LuaVersion, noenv: bool, var: &str, dft: &[u8]) -> Vec<u8> {
    if noenv {
        return dft.to_vec();
    }
    let suffix = match v {
        LuaVersion::Lua52 => "_5_2",
        LuaVersion::Lua53 => "_5_3",
        LuaVersion::Lua54 => "_5_4",
        LuaVersion::Lua55 => "_5_5",
        _ => "",
    };
    let versioned = format!("{var}{suffix}");
    let found = if suffix.is_empty() {
        std::env::var_os(var)
    } else {
        std::env::var_os(&versioned).or_else(|| std::env::var_os(var))
    };
    let Some(path) = found else {
        return dft.to_vec();
    };
    let path = os_bytes(&path);
    if v <= LuaVersion::Lua53 {
        // ";;" -> ";\1;" -> the default in place of "\1"
        let marked = replace(&path, b";;", b";\x01;");
        return replace(&marked, b"\x01", dft);
    }
    let Some(at) = path.windows(2).position(|w| w == b";;") else {
        return path;
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

pub(super) fn os_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
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
    std::fs::File::open(os_path(name)).is_ok()
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
    Ok(match r {
        Ok(file) => {
            let f = Value::Str(vm.heap.intern(&file));
            vm.nat_return(fs, &[f])
        }
        Err(msg) => {
            let m = Value::Str(vm.heap.intern(&msg));
            vm.nat_return(fs, &[Value::Nil, m])
        }
    })
}

/// `findfile`: search `package[pname]` (which must be a string) for `name`.
pub(super) fn find_file(
    vm: &mut Vm,
    pkg: Gc<Table>,
    name: &[u8],
    pname: &str,
) -> Result<Result<Vec<u8>, Vec<u8>>, LuaError> {
    let k = Value::Str(vm.heap.intern(pname.as_bytes()));
    let path = vm.index_value(Value::Table(pkg), k)?;
    let Some(path) = argcheck::to_str_bytes(vm, path) else {
        return Err(raise_str(
            vm,
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
