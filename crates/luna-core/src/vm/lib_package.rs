//! package library: `require` and the searchers it runs, `package.path` /
//! `cpath` / `config` / `loaded` / `preload`, `package.searchpath`,
//! `package.loadlib`, and 5.1/5.2's `module` / `package.seeall`. Shaped per
//! dialect after loadlib.c 5.1–5.5 (5.2 with LUA_COMPAT_MODULE and
//! LUA_COMPAT_LOADERS, as its default build has them).
//!
//! luna links no dynamic loader: `package.loadlib` and the C searchers fail
//! the way a PUC build without dynamic-library support does.

use crate::runtime::{CallFrame, Gc, Table, UserdataPayload, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::raise_str;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::isa::Op;
use crate::vm::lib_io::{c_str, os_path};

/// Default search paths. PUC compiles in its install prefix; luna has
/// none, so only the current-directory templates remain.
const LUA_PATH_DEFAULT: &[u8] = b"./?.lua;./?/init.lua";
const LUA_CPATH_DEFAULT: &[u8] = b"";

/// PUC's message and `loadlib` "where" when built without dynamic libraries.
const DLMSG: &[u8] = b"dynamic libraries not enabled; check your Lua installation";

pub(crate) fn open_package(vm: &mut Vm) {
    let v = vm.version();
    let pkg = vm.heap.new_table();
    let loaded = registry_table(vm, "_LOADED");
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
                let k = Value::Str(vm.heap.intern(name.as_bytes()));
                vm.globals().get(k)
            }
        };
        if !val.is_nil() {
            raw_set(vm, loaded, name, val);
        }
    }
    raw_set(vm, pkg, "loaded", Value::Table(loaded));
    // 5.1 keeps preload in the package table only; 5.2 moved it to the
    // registry
    let preload = if v == LuaVersion::Lua51 {
        vm.heap.new_table()
    } else {
        registry_table(vm, "_PRELOAD")
    };
    raw_set(vm, pkg, "preload", Value::Table(preload));

    let searchers = vm.heap.new_table();
    let lf = vm.native(crate::vm::lib_os_io::nat_loadfile);
    let lua_searcher = vm.native_with(searcher_lua, Box::new([Value::Table(pkg), lf]));
    let fns = [
        vm.native_with(searcher_preload, Box::new([Value::Table(pkg)])),
        lua_searcher,
        vm.native_with(searcher_c, Box::new([Value::Table(pkg)])),
        vm.native_with(searcher_croot, Box::new([Value::Table(pkg)])),
    ];
    for (i, f) in fns.into_iter().enumerate() {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { searchers.as_mut() }
            .set(&mut vm.heap, Value::Int(i as i64 + 1), f)
            .expect("valid key");
    }
    vm.barrier_back_table(searchers);
    match v {
        LuaVersion::Lua51 => raw_set(vm, pkg, "loaders", Value::Table(searchers)),
        LuaVersion::Lua52 => {
            raw_set(vm, pkg, "searchers", Value::Table(searchers));
            raw_set(vm, pkg, "loaders", Value::Table(searchers));
        }
        _ => raw_set(vm, pkg, "searchers", Value::Table(searchers)),
    }

    let path = env_path(v, "LUA_PATH", LUA_PATH_DEFAULT);
    let path = Value::Str(vm.heap.intern(&path));
    raw_set(vm, pkg, "path", path);
    let cpath = env_path(v, "LUA_CPATH", LUA_CPATH_DEFAULT);
    let cpath = Value::Str(vm.heap.intern(&cpath));
    raw_set(vm, pkg, "cpath", cpath);
    // dir separator, path separator, template mark, executable-dir mark,
    // ignore mark; 5.2 added the final newline
    let config: &[u8] = if v == LuaVersion::Lua51 {
        b"/\n;\n?\n!\n-"
    } else {
        b"/\n;\n?\n!\n-\n"
    };
    let config = Value::Str(vm.heap.intern(config));
    raw_set(vm, pkg, "config", config);

    let f = vm.native(ll_loadlib);
    raw_set(vm, pkg, "loadlib", f);
    if v >= LuaVersion::Lua52 {
        let f = vm.native(ll_searchpath);
        raw_set(vm, pkg, "searchpath", f);
    }
    // 5.1 marks a module being loaded with a sentinel, to catch loops
    let sentinel = Value::Userdata(vm.heap.new_userdata(UserdataPayload::Empty, false));
    let req = vm.native_with(
        ll_require,
        Box::new([Value::Table(pkg), Value::Table(loaded), sentinel]),
    );
    vm.set_global("require", req).expect("stdlib registration");
    if v <= LuaVersion::Lua52 {
        let f = vm.native(ll_seeall);
        raw_set(vm, pkg, "seeall", f);
        let m = vm.native_with(ll_module, Box::new([Value::Table(loaded)]));
        vm.set_global("module", m).expect("stdlib registration");
    }
    vm.set_global("package", Value::Table(pkg))
        .expect("stdlib registration");
    vm.barrier_back_table(pkg);
    vm.barrier_back_table(loaded);
}

fn raw_set(vm: &mut Vm, t: Gc<Table>, k: &str, v: Value) {
    let k = Value::Str(vm.heap.intern(k.as_bytes()));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }
        .set(&mut vm.heap, k, v)
        .expect("valid key");
}

/// `luaL_getsubtable(L, LUA_REGISTRYINDEX, name)`: the registry's table
/// `name`, created on first use. Without a registry (the debug library
/// makes it) the table is simply not reachable from there.
fn registry_table(vm: &mut Vm, name: &str) -> Gc<Table> {
    let Some(reg) = vm.registry else {
        return vm.heap.new_table();
    };
    let k = Value::Str(vm.heap.intern(name.as_bytes()));
    if let Value::Table(t) = reg.get(k) {
        return t;
    }
    let t = vm.heap.new_table();
    raw_set(vm, reg, name, Value::Table(t));
    vm.barrier_back_table(reg);
    t
}

/// `setpath`: the environment's path (5.2+ try `NAME_5_x` first) with
/// ";;" replaced by the default, else the default. 5.1–5.3 replace every
/// ";;"; 5.4 replaces the first and drops a separator left dangling.
fn env_path(v: LuaVersion, var: &str, dft: &[u8]) -> Vec<u8> {
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

fn os_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
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
fn replace(src: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
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

// ---- searching the paths ----

/// `readable`: whether `fopen(name, "r")` succeeds.
fn readable(name: &[u8]) -> bool {
    std::fs::File::open(os_path(name)).is_ok()
}

/// `searchpath`: the first readable file among `path`'s templates with
/// `name` (its `sep`s turned into `dirsep`) in place of each '?', or the
/// "no file" message listing every candidate. ≤5.3 skip empty templates
/// and start each entry with "\n\t"; 5.4 keeps empty ones and joins them.
fn search_path(
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

fn ll_searchpath(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
fn find_file(
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

/// `loaderror` / `checkload` for a module file that failed to load.
fn load_error(vm: &mut Vm, name: &[u8], file: &[u8], msg: &[u8]) -> LuaError {
    let text = format!(
        "error loading module '{}' from file '{}':\n\t{}",
        String::from_utf8_lossy(c_str(name)),
        String::from_utf8_lossy(file),
        String::from_utf8_lossy(msg)
    );
    raise_str(vm, &text)
}

fn upval_table(vm: &Vm, fs: u32, i: usize) -> Gc<Table> {
    match vm.nat_upval(fs, i) {
        Value::Table(t) => t,
        _ => unreachable!("package natives keep tables in their upvalues"),
    }
}

/// Whether `a` is the sentinel userdata `b`.
fn same(a: Value, b: Value) -> bool {
    matches!((a, b), (Value::Userdata(x), Value::Userdata(y)) if x.ptr_eq(y))
}

fn str_value(vm: &mut Vm, b: &[u8]) -> Value {
    Value::Str(vm.heap.intern(b))
}

// ---- the searchers ----

fn searcher_preload(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let preload = if v == LuaVersion::Lua51 {
        let pkg = upval_table(vm, fs, 0);
        let k = Value::Str(vm.heap.intern(b"preload"));
        match vm.index_value(Value::Table(pkg), k)? {
            Value::Table(t) => t,
            _ => return Err(raise_str(vm, "'package.preload' must be a table")),
        }
    } else {
        registry_table(vm, "_PRELOAD")
    };
    let loader = vm.index_value(Value::Table(preload), Value::Str(name))?;
    if loader.is_nil() {
        let lead = if v >= LuaVersion::Lua54 { "" } else { "\n\t" };
        let msg = format!(
            "{lead}no field package.preload['{}']",
            String::from_utf8_lossy(c_str(name.as_bytes()))
        );
        let m = str_value(vm, msg.as_bytes());
        return Ok(vm.nat_return(fs, &[m]));
    }
    if v >= LuaVersion::Lua54 {
        let data = str_value(vm, b":preload:");
        return Ok(vm.nat_return(fs, &[loader, data]));
    }
    Ok(vm.nat_return(fs, &[loader]))
}

fn searcher_lua(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let pkg = upval_table(vm, fs, 0);
    let file = match find_file(vm, pkg, &name, "path")? {
        Ok(f) => f,
        Err(msg) => {
            let m = str_value(vm, &msg);
            return Ok(vm.nat_return(fs, &[m]));
        }
    };
    let loadfile = vm.nat_upval(fs, 1);
    let fname = str_value(vm, &file);
    let r = vm.call_value(loadfile, &[fname])?;
    match r.first() {
        Some(f @ Value::Closure(_)) => {
            let f = *f;
            // 5.1's loader gets only the name; 5.2+ also the file name
            if vm.version() == LuaVersion::Lua51 {
                return Ok(vm.nat_return(fs, &[f]));
            }
            Ok(vm.nat_return(fs, &[f, fname]))
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
fn searcher_c(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let pkg = upval_table(vm, fs, 0);
    match find_file(vm, pkg, &name, "cpath")? {
        Ok(file) => Err(load_error(vm, &name, &file, DLMSG)),
        Err(msg) => {
            let m = str_value(vm, &msg);
            Ok(vm.nat_return(fs, &[m]))
        }
    }
}

/// The all-in-one C searcher: look for the root of a dotted name on
/// `cpath`; a name without a dot is not its business.
fn searcher_croot(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name = argcheck::check_string(vm, Args::new(fs, nargs), 0)?
        .as_bytes()
        .to_vec();
    let Some(dot) = c_str(&name).iter().position(|&b| b == b'.') else {
        return Ok(vm.nat_return(fs, &[]));
    };
    let pkg = upval_table(vm, fs, 0);
    match find_file(vm, pkg, &name[..dot], "cpath")? {
        Ok(file) => Err(load_error(vm, &name, &file, DLMSG)),
        Err(msg) => {
            let m = str_value(vm, &msg);
            Ok(vm.nat_return(fs, &[m]))
        }
    }
}

fn ll_loadlib(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    argcheck::check_string(vm, a, 0)?;
    argcheck::check_string(vm, a, 1)?;
    let msg = str_value(vm, DLMSG);
    let place = str_value(vm, b"absent");
    Ok(vm.nat_return(fs, &[Value::Nil, msg, place]))
}

// ---- require ----

/// `findloader`: ask each searcher in turn; the first one to return a
/// function supplies the loader (and its extra value). The misses are
/// collected into the "not found" message.
fn find_loader(vm: &mut Vm, pkg: Gc<Table>, name: Value) -> Result<(Value, Value), LuaError> {
    let v = vm.version();
    let field = if v == LuaVersion::Lua51 {
        "loaders"
    } else {
        "searchers"
    };
    let k = Value::Str(vm.heap.intern(field.as_bytes()));
    let Value::Table(searchers) = vm.index_value(Value::Table(pkg), k)? else {
        return Err(raise_str(vm, &format!("'package.{field}' must be a table")));
    };
    let mut msg = Vec::new();
    for i in 1.. {
        let s = searchers.get(Value::Int(i));
        if s.is_nil() {
            break;
        }
        let r = vm.call_value(s, &[name])?;
        let first = r.first().copied().unwrap_or(Value::Nil);
        let second = r.get(1).copied().unwrap_or(Value::Nil);
        if matches!(first, Value::Closure(_) | Value::Native(_)) {
            return Ok((first, second));
        }
        if let Some(text) = argcheck::to_str_bytes(vm, first) {
            // 5.4 puts the separator between messages itself
            if v >= LuaVersion::Lua54 {
                msg.extend_from_slice(b"\n\t");
            }
            msg.extend_from_slice(&text);
        }
    }
    let Value::Str(n) = name else {
        unreachable!("require passes the name as a string");
    };
    let text = format!(
        "module '{}' not found:{}",
        String::from_utf8_lossy(c_str(n.as_bytes())),
        String::from_utf8_lossy(&msg)
    );
    Err(raise_str(vm, &text))
}

fn ll_require(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let name_s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let name = Value::Str(name_s);
    let pkg = upval_table(vm, fs, 0);
    let loaded = upval_table(vm, fs, 1);
    let sentinel = vm.nat_upval(fs, 2);
    let cur = vm.index_value(Value::Table(loaded), name)?;
    if cur.truthy() {
        if v == LuaVersion::Lua51 && same(cur, sentinel) {
            let text = format!(
                "loop or previous error loading module '{}'",
                String::from_utf8_lossy(c_str(name_s.as_bytes()))
            );
            return Err(raise_str(vm, &text));
        }
        return Ok(vm.nat_return(fs, &[cur]));
    }
    let (loader, data) = find_loader(vm, pkg, name)?;
    if v == LuaVersion::Lua51 {
        vm.newindex_value(Value::Table(loaded), name, sentinel)?;
    }
    // 5.1 hands the loader only the name
    let args: &[Value] = if v == LuaVersion::Lua51 {
        &[name]
    } else {
        &[name, data]
    };
    let r = vm.call_value(loader, args)?;
    let res = r.first().copied().unwrap_or(Value::Nil);
    if !res.is_nil() {
        vm.newindex_value(Value::Table(loaded), name, res)?;
    }
    let mut value = vm.index_value(Value::Table(loaded), name)?;
    let unset = if v == LuaVersion::Lua51 {
        same(value, sentinel)
    } else {
        value.is_nil()
    };
    if unset {
        value = Value::Bool(true);
        vm.newindex_value(Value::Table(loaded), name, value)?;
    }
    // 5.4 also returns the loader data
    if v >= LuaVersion::Lua54 {
        return Ok(vm.nat_return(fs, &[value, data]));
    }
    Ok(vm.nat_return(fs, &[value]))
}

// ---- module / seeall (5.1, 5.2) ----

/// `luaL_findtable` on the globals: walk (raw) the dotted `name`, creating
/// missing tables; `None` when a part is a non-table value.
fn find_table(vm: &mut Vm, name: &[u8]) -> Option<Gc<Table>> {
    let mut t = vm.globals();
    for part in name.split(|&b| b == b'.') {
        let k = Value::Str(vm.heap.intern(part));
        t = match t.get(k) {
            Value::Table(next) => next,
            Value::Nil => {
                let next = vm.heap.new_table();
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { t.as_mut() }
                    .set(&mut vm.heap, k, Value::Table(next))
                    .expect("valid key");
                vm.barrier_back_table(t);
                next
            }
            _ => return None,
        };
    }
    Some(t)
}

fn ll_module(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let a = Args::new(fs, nargs);
    let name_s = argcheck::check_string(vm, a, 0)?;
    let name = c_str(name_s.as_bytes()).to_vec();
    let loaded = upval_table(vm, fs, 0);
    let module = match loaded.get(Value::Str(name_s)) {
        Value::Table(t) => t,
        _ => {
            let Some(t) = find_table(vm, &name) else {
                let text = format!(
                    "name conflict for module '{}'",
                    String::from_utf8_lossy(&name)
                );
                return Err(raise_str(vm, &text));
            };
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { loaded.as_mut() }
                .set(&mut vm.heap, Value::Str(name_s), Value::Table(t))
                .expect("valid key");
            vm.barrier_back_table(loaded);
            t
        }
    };
    let mv = Value::Table(module);
    let nk = str_value(vm, b"_NAME");
    if vm.index_value(mv, nk)?.is_nil() {
        // modinit: _M, _NAME, and _PACKAGE (the name up to its last dot)
        let mk = str_value(vm, b"_M");
        vm.newindex_value(mv, mk, mv)?;
        let nv = str_value(vm, &name);
        vm.newindex_value(mv, nk, nv)?;
        let cut = name.iter().rposition(|&b| b == b'.').map_or(0, |i| i + 1);
        let pk = str_value(vm, b"_PACKAGE");
        let pv = str_value(vm, &name[..cut]);
        vm.newindex_value(mv, pk, pv)?;
    }
    set_caller_env(vm, mv)?;
    // options: 5.1 calls every extra argument, 5.2 only the functions
    for i in 1..nargs {
        let opt = a.get(vm, i);
        if v == LuaVersion::Lua52 && !matches!(opt, Value::Closure(_) | Value::Native(_)) {
            continue;
        }
        vm.call_value(opt, &[mv])?;
    }
    if v == LuaVersion::Lua51 {
        return Ok(vm.nat_return(fs, &[]));
    }
    Ok(vm.nat_return(fs, &[mv]))
}

/// Make `env` the environment of the Lua function that called `module`:
/// 5.1's `setfenv` rewrites its (per-closure) `_ENV` cell, 5.2's
/// `lua_setupvalue(f, 1)` its first upvalue.
fn set_caller_env(vm: &mut Vm, env: Value) -> Result<(), LuaError> {
    let Some(cl) = lua_caller(vm) else {
        return Err(raise_str(vm, "'module' not called from a Lua function"));
    };
    let idx = if vm.version() == LuaVersion::Lua51 {
        cl.proto.upvals.iter().position(|d| &*d.name == "_ENV")
    } else {
        (!cl.upvals().is_empty()).then_some(0)
    };
    if let Some(i) = idx {
        vm.upvalue_set_value(cl, i, env);
    }
    Ok(())
}

/// The Lua function that called the running native (PUC: level 1 of the
/// stack is a Lua activation), if it was one. A Lua caller is stopped at the
/// call instruction whose function register is this native's slot; a native
/// called from another native or from a pcall continuation fails that test.
fn lua_caller(vm: &Vm) -> Option<Gc<crate::runtime::LuaClosure>> {
    let &(slot, _) = vm.running_native_slots.last()?;
    let CallFrame::Lua(f) = vm.inspect_frames().last()? else {
        return None;
    };
    let call = *f.closure.proto.code.get((f.pc as usize).checked_sub(1)?)?;
    (matches!(call.op(), Op::Call | Op::TailCall) && f.base + call.a() == slot).then_some(f.closure)
}

fn ll_seeall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = argcheck::check_table(vm, Args::new(fs, nargs), 0)?;
    let mt = match t.metatable() {
        Some(mt) => mt,
        None => {
            let mt = vm.heap.new_table();
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { t.as_mut() }.set_metatable(Some(mt));
            mt
        }
    };
    let g = Value::Table(vm.globals());
    raw_set(vm, mt, "__index", g);
    vm.barrier_back_table(mt);
    Ok(vm.nat_return(fs, &[]))
}
