//! The searchers `require` runs, and `package.loadlib`.

use super::*;

/// `loaderror` / `checkload` for a module file that failed to load.
pub(super) fn load_error(vm: &mut Vm, name: &[u8], file: &[u8], msg: &[u8]) -> LuaError {
    let args = [
        crate::runtime::puc::fmt::FmtArg::S(c_str(name)),
        crate::runtime::puc::fmt::FmtArg::S(file),
        crate::runtime::puc::fmt::FmtArg::S(msg),
    ];
    let fmt = b"error loading module '%s' from file '%s':\n\t%s";
    crate::vm::builtins::raise_lib(vm, fmt, &args, 3)
}

pub(super) fn searcher_preload(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let preload = if v == LuaVersion::Lua51 {
        let pkg = upval_table(vm, fs, 0);
        match vm.native_getfield(Value::Table(pkg), None, b"preload")? {
            Value::Table(t) => t,
            _ => return Err(raise_str(vm, "'package.preload' must be a table")),
        }
    } else {
        let t = registry_table(vm, "_PRELOAD")?;
        vm.native_push_vals(&[Value::Table(t)])?;
        t
    };
    let loader = vm.native_getfield(
        Value::Table(preload),
        Some(super::require::name_at(name)),
        name.as_bytes(),
    )?;
    if loader.is_nil() {
        let fmt: &[u8] = if v >= LuaVersion::Lua54 {
            b"no field package.preload['%s']"
        } else {
            b"\n\tno field package.preload['%s']"
        };
        let args = [crate::runtime::puc::fmt::FmtArg::S(c_str(name.as_bytes()))];
        let m = with_rules!(vm, P => vm.api_pushfstring::<P>(1, fmt, &args))?;
        return vm.nat_ret(fs, &[Value::Str(m)]);
    }
    if v >= LuaVersion::Lua54 {
        let data = vm.api_push_string_dyn(b":preload:")?;
        return vm.nat_ret(fs, &[loader, data]);
    }
    vm.nat_ret(fs, &[loader])
}

pub(super) fn searcher_lua(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let pkg = upval_table(vm, fs, 0);
    let found = match find_file(vm, pkg, &name, "path")? {
        Ok(f) => f,
        Err(msg) => return vm.nat_ret(fs, &[Value::Str(msg)]),
    };
    let file = found.file.clone();
    let fname = super::path::found_string(vm, &found)?;
    // `luaL_loadfilex`, a C function the searcher calls (no level of its
    // own), the name found on the stack meanwhile
    let base = vm.lib_roots.mark();
    vm.lib_roots.push(fname);
    let r = crate::vm::lib_os_io::load_path(vm, Some(&file), None);
    vm.lib_roots.release(base);
    match r {
        Ok(f @ Value::Closure(_)) => {
            // 5.1's loader gets only the name; 5.2+ also the file name,
            // which `checkload` pushes again from the found name's bytes
            if vm.version() == LuaVersion::Lua51 {
                found.returned(vm, &[f])?;
                return vm.nat_ret(fs, &[f]);
            }
            let Value::Str(fstr) = fname else {
                unreachable!("a string")
            };
            vm.lib_roots.extend([f, fname]);
            let at = crate::runtime::puc::records::strcache::CStr::In(fstr.as_ptr() as *const _, 0);
            let pushed = with_rules!(vm, P => vm.api_push_string_at::<P>(at, &file));
            vm.lib_roots.pop_n(2);
            let pushed = pushed?;
            found.returned(vm, &[f, pushed])?;
            vm.nat_ret(fs, &[f, pushed])
        }
        Ok(m) | Err(m) => {
            let msg = argcheck::to_str_bytes(vm, m).unwrap_or_default();
            Err(load_error(vm, &name, &file, &msg))
        }
    }
}

/// The C searchers: a file found on `cpath` cannot be opened without a
/// dynamic loader, which is a load error.
pub(super) fn searcher_c(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let pkg = upval_table(vm, fs, 0);
    match find_file(vm, pkg, &name, "cpath")? {
        Ok(f) => Err(load_error(vm, &name, &f.file, DLMSG)),
        Err(msg) => vm.nat_ret(fs, &[Value::Str(msg)]),
    }
}

/// The all-in-one C searcher: look for the root of a dotted name on
/// `cpath`; a name without a dot is not its business.
pub(super) fn searcher_croot(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let Some(dot) = c_str(&name).iter().position(|&b| b == b'.') else {
        return vm.nat_ret(fs, &[]);
    };
    let pkg = upval_table(vm, fs, 0);
    // the root is pushed, and searched for from there
    let root = vm.api_push_string_dyn(&name[..dot])?;
    let found = vm.rooted(&[root], |vm| find_file(vm, pkg, &name[..dot], "cpath"));
    match found? {
        Ok(f) => Err(load_error(vm, &name, &f.file, DLMSG)),
        Err(msg) => vm.nat_ret(fs, &[Value::Str(msg)]),
    }
}

pub(super) fn ll_loadlib(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    argcheck::check_string(vm, a, 0)?;
    argcheck::check_string(vm, a, 1)?;
    let msg = str_value(vm, DLMSG)?;
    let place = str_value(vm, b"absent")?;
    vm.nat_ret(fs, &[Value::Nil, msg, place])
}
