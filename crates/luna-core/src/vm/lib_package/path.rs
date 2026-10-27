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

/// PUC's `LUA_CPATH_DEFAULT` outside Windows, its install prefix
/// included (Windows' is in `progdir`). luna finds C libraries there but
/// loads none.
pub(super) fn cpath_default(v: LuaVersion) -> Vec<u8> {
    let vdir = match v {
        LuaVersion::Lua51 => "5.1",
        LuaVersion::Lua52 => "5.2",
        LuaVersion::Lua53 => "5.3",
        LuaVersion::Lua54 => "5.4",
        _ => "5.5",
    };
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

pub(super) fn ll_searchpath(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let [name, path, sep, dirsep] = searchpath_args(vm, a)?;
    let r =
        super::search_acct::search(vm, c_str(&name), c_str(&path), c_str(&sep), c_str(&dirsep))?;
    match r {
        Ok(f) => {
            let s = found_string(vm, &f)?;
            f.returned(vm, &[s])?;
            vm.nat_ret(fs, &[s])
        }
        Err(msg) => vm.nat_ret(fs, &[Value::Nil, Value::Str(msg)]),
    }
}

/// The string of `file`, the name `searchpath` found: 5.4 on pushed from
/// byte `at` of its buffer, where that is in its frame (see `strcache`),
/// ≤5.3 made by `luaL_gsub`.
pub(super) fn found_string(vm: &mut Vm, f: &super::search_acct::Found) -> Result<Value, LuaError> {
    use crate::runtime::puc::records::strcache::{CStr, site};
    match (f.made, f.at) {
        (Some(s), _) => Ok(Value::Str(s)),
        (None, Some(off)) => vm.str_value_at(
            CStr::Frame(site::SEARCHPATH, vm.c_chain_key(), off),
            &f.file,
        ),
        (None, None) => vm.str_value(&f.file),
    }
}

/// `findfile`: search `package[pname]` (which must be a string) for `name`.
pub(super) fn find_file(
    vm: &mut Vm,
    pkg: Gc<Table>,
    name: &[u8],
    pname: &str,
) -> Result<Result<super::search_acct::Found, Gc<crate::runtime::LuaStr>>, LuaError> {
    let path = vm.native_getfield(Value::Table(pkg), None, pname.as_bytes())?;
    let Some(path) = argcheck::to_str_bytes(vm, path) else {
        let args = [crate::runtime::puc::fmt::FmtArg::S(pname.as_bytes())];
        let fmt = b"'package.%s' must be a string";
        return Err(crate::vm::builtins::raise_lib(vm, fmt, &args, 1));
    };
    super::search_acct::search(vm, c_str(name), c_str(&path), b".", LUA_DIRSEP)
}

/// The arguments of `package.searchpath`, all four checked in one call's
/// arguments: from the first on.
#[cfg(not(target_arch = "x86"))]
fn searchpath_args(vm: &mut Vm, a: Args) -> Result<[Vec<u8>; 4], LuaError> {
    let name = argcheck::check_string(vm, a, 0)?.as_bytes().to_vec();
    let path = argcheck::check_string(vm, a, 1)?.as_bytes().to_vec();
    let sep = opt_bytes(vm, a, 2, b".")?;
    Ok([name, path, sep, opt_bytes(vm, a, 3, LUA_DIRSEP)?])
}

/// [`searchpath_args`] where gcc evaluates a call's arguments from the
/// last one on (i386).
#[cfg(target_arch = "x86")]
fn searchpath_args(vm: &mut Vm, a: Args) -> Result<[Vec<u8>; 4], LuaError> {
    let dirsep = opt_bytes(vm, a, 3, LUA_DIRSEP)?;
    let sep = opt_bytes(vm, a, 2, b".")?;
    let path = argcheck::check_string(vm, a, 1)?.as_bytes().to_vec();
    Ok([
        argcheck::check_string(vm, a, 0)?.as_bytes().to_vec(),
        path,
        sep,
        dirsep,
    ])
}

/// `luaL_optstring(L, i + 1, def)`'s bytes.
fn opt_bytes(vm: &mut Vm, a: Args, i: u32, def: &[u8]) -> Result<Vec<u8>, LuaError> {
    Ok(match argcheck::opt_string(vm, a, i)? {
        Some(s) => s.as_bytes().to_vec(),
        None => def.to_vec(),
    })
}
