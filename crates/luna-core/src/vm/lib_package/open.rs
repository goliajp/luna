//! `luaopen_package` from 5.2 on, in PUC's order: the tables of the sizes
//! PUC makes them, filled in the order PUC fills them, so the count and
//! the table growth follow PUC's when the library is opened counted (a
//! library opened again, or one at a time).

use super::*;

/// The fields `luaL_newlib(L, pk_funcs)` sizes the package table for:
/// 5.2's functions only, later ones with placeholders for the fields set
/// after.
fn pk_funcs(v: LuaVersion) -> usize {
    if v == LuaVersion::Lua52 { 3 } else { 7 }
}

/// Open `package` (5.2 on); with `list_opened`, `package.loaded` lists the
/// libraries opened before it (luna's own first opening, which counts
/// nothing).
pub(super) fn open_package_puc(vm: &mut Vm, list_opened: bool) -> Result<(), LuaError> {
    let v = vm.version();
    clibs(vm)?;
    let pkg = vm.lib_table(pk_funcs(v))?;
    let f = vm.lib_native(ll_loadlib)?;
    raw_set(vm, pkg, "loadlib", f)?;
    let f = vm.lib_native(ll_searchpath)?;
    raw_set(vm, pkg, "searchpath", f)?;
    if v == LuaVersion::Lua52 {
        let f = vm.lib_native(ll_seeall)?;
        raw_set(vm, pkg, "seeall", f)?;
    } else {
        // `luaL_setfuncs` gives each placeholder `false`
        for k in ["preload", "cpath", "path", "searchers", "loaded"] {
            raw_set(vm, pkg, k, Value::Bool(false))?;
        }
    }
    let searchers = vm.alloc_table_sized(4, 0)?;
    let lf = vm.lib_native(crate::vm::lib_os_io::nat_loadfile)?;
    let p = Value::Table(pkg);
    let fns = [
        puc_closure(vm, searcher_preload, &[p])?,
        puc_closure(vm, searcher_lua, &[p, lf])?,
        puc_closure(vm, searcher_c, &[p])?,
        puc_closure(vm, searcher_croot, &[p])?,
    ];
    for (i, f) in fns.into_iter().enumerate() {
        vm.lib_set_value(searchers, Value::Int(i as i64 + 1), f)?;
    }
    vm.barrier_back_table(searchers);
    if v == LuaVersion::Lua52 {
        raw_set(vm, pkg, "loaders", Value::Table(searchers))?;
    }
    raw_set(vm, pkg, "searchers", Value::Table(searchers))?;
    set_paths(vm, pkg)?;
    let config = vm.str_value(b"/\n;\n?\n!\n-\n")?;
    raw_set(vm, pkg, "config", config)?;
    let loaded = registry_table(vm, "_LOADED")?;
    if list_opened {
        list_libs(vm, loaded, pkg)?;
    }
    raw_set(vm, pkg, "loaded", Value::Table(loaded))?;
    let preload = registry_table(vm, "_PRELOAD")?;
    raw_set(vm, pkg, "preload", Value::Table(preload))?;
    // 5.1 marks a module being loaded with a sentinel; PUC's is a light
    // userdata, nothing it counts
    vm.heap.puc.silent += 1;
    let sentinel = vm.lib_userdata(UserdataPayload::Empty, false, 0);
    vm.heap.puc.silent -= 1;
    let sentinel = Value::Userdata(sentinel?);
    if v == LuaVersion::Lua52 {
        let m = puc_closure(vm, ll_module, &[Value::Table(loaded)])?;
        vm.lib_set_global("module", m)?;
    }
    let req = puc_closure(vm, ll_require, &[p, Value::Table(loaded), sentinel])?;
    vm.lib_set_global("require", req)?;
    vm.lib_set_global("package", p)?;
    vm.barrier_back_table(pkg);
    vm.barrier_back_table(loaded);
    Ok(())
}

/// A function PUC makes as a C closure with the package table as its one
/// upvalue (`lua_pushcclosure`, which checks for a collection); luna's
/// keeps `upvals`. Opened without counting, the record has PUC's.
fn puc_closure(
    vm: &mut Vm,
    f: crate::runtime::value::NativeFn,
    upvals: &[Value],
) -> Result<Value, LuaError> {
    if vm.heap.puc.silent > 0 {
        vm.lib_native_with(f, upvals)
    } else {
        vm.lib_native_puc(f, upvals, 1)
    }
}

/// The table of loaded C libraries and its metatable (`createclibstable`):
/// 5.2 and 5.4 find the table in the registry, 5.3 makes a new one; 5.5
/// keeps it without a metatable. luna's own first opening makes the
/// registry's after the libraries (`Vm::puc_registry`).
fn clibs(vm: &mut Vm) -> Result<(), LuaError> {
    let v = vm.version();
    if v == LuaVersion::Lua55 || vm.heap.puc.silent > 0 {
        return Ok(());
    }
    let clibs = if v == LuaVersion::Lua53 {
        let t = vm.alloc_table()?;
        if let Some(reg) = vm.registry {
            let k = crate::vm::exec::light_key(&crate::vm::exec::CLIBS_KEY);
            vm.lib_set_value(reg, k, Value::Table(t))?;
        }
        t
    } else {
        registry_table(vm, "_CLIBS")?
    };
    clibs_metatable(vm, clibs)
}

/// `clibs` gets a new metatable with the finalizer that would unload the
/// libraries.
pub(crate) fn clibs_metatable(vm: &mut Vm, clibs: Gc<Table>) -> Result<(), LuaError> {
    let mt = vm.lib_table(1)?;
    let gc = vm.lib_native(clibs_gc)?;
    raw_set(vm, mt, "__gc", gc)?;
    // SAFETY: `clibs` is a live table the registry holds; the borrow covers one store
    unsafe { clibs.as_mut() }.set_metatable(Some(mt));
    vm.barrier_back_table(clibs);
    vm.check_finalizer(clibs)
}

/// The C libraries' finalizer: luna loads none.
fn clibs_gc(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Ok(0)
}

/// The libraries opened before `package`, in `loaded`.
fn list_libs(vm: &mut Vm, loaded: Gc<Table>, pkg: Gc<Table>) -> Result<(), LuaError> {
    for name in [
        "_G",
        "package",
        "coroutine",
        "table",
        "io",
        "os",
        "string",
        "bit32",
        "math",
        "utf8",
        "debug",
    ] {
        let val = match name {
            "package" => Value::Table(pkg),
            _ => {
                let k = vm.str_value(name.as_bytes())?;
                vm.globals().get(k)
            }
        };
        if !val.is_nil() {
            raw_set(vm, loaded, name, val)?;
        }
    }
    Ok(())
}

/// `package.path` and `package.cpath` (`setpath`), with the strings PUC
/// makes on the way: the versioned variable names (5.3, 5.4) and the
/// paths themselves. luna's default Lua path is its own; counted, it
/// stands for PUC's.
fn set_paths(vm: &mut Vm, pkg: Gc<Table>) -> Result<(), LuaError> {
    let v = vm.version();
    let noenv = vm.ignore_env;
    for (field, var) in [("path", "LUA_PATH"), ("cpath", "LUA_CPATH")] {
        let suffix = match v {
            LuaVersion::Lua53 => "_5_3",
            LuaVersion::Lua54 => "_5_4",
            _ => "",
        };
        if !suffix.is_empty() && vm.heap.puc.silent == 0 {
            if v == LuaVersion::Lua53 {
                // `lua_pushfstring(L, "%s%s", ...)` pushes each piece
                vm.str_value(b"")?;
                vm.str_value(var.as_bytes())?;
                vm.str_value(suffix.as_bytes())?;
            }
            vm.str_value(format!("{var}{suffix}").as_bytes())?;
        }
        let dft = if field == "path" {
            LUA_PATH_DEFAULT.to_vec()
        } else {
            cpath_default(v)
        };
        let bytes = env_path(v, noenv, var, &dft);
        let s = path_string(vm, pkg, field, &bytes)?;
        raw_set(vm, pkg, field, s)?;
    }
    Ok(())
}

/// The string `bytes` of field `field`, made as PUC makes it: 5.5 keeps
/// its defaults as external strings over its own bytes (a header each);
/// luna's own default Lua path stands for PUC's.
fn path_string(vm: &mut Vm, pkg: Gc<Table>, field: &str, bytes: &[u8]) -> Result<Value, LuaError> {
    let v = vm.version();
    if vm.heap.puc.silent > 0 {
        return vm.str_value(bytes);
    }
    let puc = vm.heap.puc_default_path();
    let stands = v == LuaVersion::Lua55
        || (field == "path" && puc.is_some_and(|p| p != bytes) && bytes == LUA_PATH_DEFAULT);
    if !stands {
        return vm.str_value(bytes);
    }
    vm.heap.puc.silent += 1;
    let s = vm.str_value(bytes);
    vm.heap.puc.silent -= 1;
    let Value::Str(s) = s? else {
        unreachable!("a string value");
    };
    let size = if v == LuaVersion::Lua55 {
        vm.heap.sizes.lngstr_hdr
    } else {
        let len = puc.map_or(bytes.len(), <[u8]>::len);
        vm.heap.sizes.string(len, true)
    };
    vm.heap.acct_stand_on(pkg.header(), size, true);
    Ok(Value::Str(s))
}
