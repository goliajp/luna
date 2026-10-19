//! The searchers `require` runs, and `package.loadlib`.

use super::*;

/// `loaderror` / `checkload` for a module file that failed to load.
pub(super) fn load_error(vm: &mut Vm, name: &[u8], file: &[u8], msg: &[u8]) -> LuaError {
    let text = format!(
        "error loading module '{}' from file '{}':\n\t{}",
        String::from_utf8_lossy(c_str(name)),
        String::from_utf8_lossy(file),
        String::from_utf8_lossy(msg)
    );
    // "error loading module '%s' from file '%s':\n\t%s"
    crate::vm::builtins::raise_fmt(vm, 3, &text)
}

pub(super) fn searcher_preload(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let preload = if v == LuaVersion::Lua51 {
        let pkg = upval_table(vm, fs, 0);
        match vm.native_getfield(Value::Table(pkg), b"preload")? {
            Value::Table(t) => t,
            _ => return Err(raise_str(vm, "'package.preload' must be a table")),
        }
    } else {
        let t = registry_table(vm, "_PRELOAD")?;
        vm.native_push_vals(&[Value::Table(t)])?;
        t
    };
    let loader = vm.native_getfield(Value::Table(preload), name.as_bytes())?;
    if loader.is_nil() {
        let lead = if v >= LuaVersion::Lua54 { "" } else { "\n\t" };
        let msg = format!(
            "{lead}no field package.preload['{}']",
            String::from_utf8_lossy(c_str(name.as_bytes()))
        );
        let m = vm.api_push_string(msg.as_bytes())?;
        return vm.nat_ret(fs, &[m]);
    }
    if v >= LuaVersion::Lua54 {
        let data = vm.api_push_string(b":preload:")?;
        return vm.nat_ret(fs, &[loader, data]);
    }
    vm.nat_ret(fs, &[loader])
}

pub(super) fn searcher_lua(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let pkg = upval_table(vm, fs, 0);
    let file = match find_file(vm, pkg, &name, "path")? {
        Ok(f) => f,
        Err(msg) => {
            let m = str_value(vm, &msg)?;
            return vm.nat_ret(fs, &[m]);
        }
    };
    let loadfile = vm.nat_upval(fs, 1);
    let fname = str_value(vm, &file)?;
    let r = vm.call_value(loadfile, &[fname])?;
    match r.first() {
        Some(f @ Value::Closure(_)) => {
            let f = *f;
            // 5.1's loader gets only the name; 5.2+ also the file name
            if vm.version() == LuaVersion::Lua51 {
                return vm.nat_ret(fs, &[f]);
            }
            vm.nat_ret(fs, &[f, fname])
        }
        _ => {
            let msg = match r.get(1) {
                Some(&m) => argcheck::to_str_bytes(vm, m).unwrap_or_default(),
                None => Vec::new(),
            };
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
        Ok(file) => Err(load_error(vm, &name, &file, DLMSG)),
        Err(msg) => {
            let m = str_value(vm, &msg)?;
            vm.nat_ret(fs, &[m])
        }
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
    match find_file(vm, pkg, &name[..dot], "cpath")? {
        Ok(file) => Err(load_error(vm, &name, &file, DLMSG)),
        Err(msg) => {
            let m = str_value(vm, &msg)?;
            vm.nat_ret(fs, &[m])
        }
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
