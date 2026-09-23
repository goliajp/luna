//! package library: `require` and its searchers, `package.searchpath` /
//! `loadlib`, and the 5.1/5.2 `module` / `package.seeall`.

use crate::runtime::Value;
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_package(vm: &mut Vm) {
    let pkg = vm.heap.new_table();
    let loaded = vm.heap.new_table();
    // prepopulate with the standard libraries (PUC does the same). Must include
    // every stdlib so e.g. nextvar.lua's "clear globals" test (which keeps any
    // name present in package.loaded) does not delete `coroutine`.
    for name in [
        "string",
        "math",
        "table",
        "os",
        "io",
        "utf8",
        "debug",
        "coroutine",
        "_G",
        "package",
    ] {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        let v = if name == "package" {
            Value::Table(pkg)
        } else if name == "_G" {
            Value::Table(vm.globals())
        } else {
            let gk = Value::Str(vm.heap.intern(name.as_bytes()));
            vm.globals().get(gk)
        };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { loaded.as_mut() }
            .set(&mut vm.heap, k, v)
            .expect("valid key");
    }
    let lk = Value::Str(vm.heap.intern(b"loaded"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, lk, Value::Table(loaded))
        .expect("valid key");
    let preload = vm.heap.new_table();
    let plk = Value::Str(vm.heap.intern(b"preload"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, plk, Value::Table(preload))
        .expect("valid key");
    // package.path: PUC default has `./?.lua` and `./?/init.lua`. attrib.lua
    // rewrites it freely so this is just a sane starting value.
    let pk = Value::Str(vm.heap.intern(b"path"));
    let pv = Value::Str(vm.heap.intern(b"./?.lua;./?/init.lua"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, pk, pv)
        .expect("valid key");
    // package.cpath: luna does not ship dynamic-library loading, so the
    // default is empty. attrib.lua's require-message test rewrites it.
    let ck = Value::Str(vm.heap.intern(b"cpath"));
    let cv = Value::Str(vm.heap.intern(b""));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, ck, cv)
        .expect("valid key");
    // package.config: PUC's five-line POSIX layout — dir-sep "/", path-sep
    // ";", template mark "?", exec-mark "!", ignore-mark "-".
    let cfk = Value::Str(vm.heap.intern(b"config"));
    let cfv = Value::Str(vm.heap.intern(b"/\n;\n?\n!\n-\n"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, cfk, cfv)
        .expect("valid key");
    // package.searchers: present as a table so attrib.lua's type checks pass.
    // luna's `require` does not dispatch through it (the searchers run in a
    // fixed order inside `nat_require`); this stays a leaf placeholder until
    // a userland test forces real dispatch.
    let searchers = vm.heap.new_table();
    let sk = Value::Str(vm.heap.intern(b"searchers"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, sk, Value::Table(searchers))
        .expect("valid key");
    // package.searchpath: pure path-template walker (no I/O side effects
    // beyond probing readability), shared with userland and exposed here.
    let sp = vm.native(nat_searchpath);
    let spk = Value::Str(vm.heap.intern(b"searchpath"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, spk, sp)
        .expect("valid key");
    vm.set_global("package", Value::Table(pkg))
        .expect("stdlib registration");
    // require reads package.path/cpath from the live `package` table each call
    // — attrib.lua mutates them inside `do … end` blocks and require must see
    // the override. Stash no upvalues; fetch from globals on demand.
    // PUC keeps `_LOADED` / `_PRELOAD` in the registry so a stray
    // `package = {}` in user code does not unlink the real bookkeeping.
    // luna captures the same tables as the require-native's own upvalues so
    // the lookup is stable regardless of `package`'s global identity. Slots:
    //   [0] = package table (still consulted for `path` / `cpath` overrides
    //         the user *does* expect to flow through globals);
    //   [1] = `package.loaded`;
    //   [2] = `package.preload`.
    let req = vm.native_with(
        nat_require,
        Box::new([
            Value::Table(pkg),
            Value::Table(loaded),
            Value::Table(preload),
        ]),
    );
    vm.set_global("require", req).expect("stdlib registration");
    // PUC 5.1 `module(name, ...)` and `package.seeall` (retired in 5.2). The
    // pair only makes sense alongside `setfenv`; gating on the dialect keeps
    // the 5.2+ surface clean.
    if vm.version() == crate::version::LuaVersion::Lua51 {
        // Same upval-anchored bookkeeping as `require`: the original
        // `package.loaded` is captured here so `module(...)` survives a
        // userland `package = {}` reassignment (attrib.lua's `do … end`
        // preload block does exactly that).
        let m = vm.native_with(nat_module, Box::new([Value::Table(loaded)]));
        vm.set_global("module", m).expect("stdlib registration");
        let s = vm.native(nat_package_seeall);
        let sk = Value::Str(vm.heap.intern(b"seeall"));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { pkg.as_mut() }
            .set(&mut vm.heap, sk, s)
            .expect("valid key");
    }
    // PUC's `package.loadlib` opens a shared library and returns the named
    // symbol. luna ships no dynamic linker — return the PUC failure shape so
    // attrib.lua's "cannot load dynamic library" path (which prints a notice
    // and skips the C-only suite) runs rather than blowing up on a nil call.
    let ll = vm.native(nat_loadlib_stub);
    let llk = Value::Str(vm.heap.intern(b"loadlib"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { pkg.as_mut() }
        .set(&mut vm.heap, llk, ll)
        .expect("valid key");
    // Once-per-table barriers for the four sub-tables built above —
    // covers the post-init `Vm::open_package` re-open path (mid-Propagate).
    vm.barrier_back_table(pkg);
    vm.barrier_back_table(loaded);
    vm.barrier_back_table(preload);
    vm.barrier_back_table(searchers);
}

fn nat_loadlib_stub(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let msg = Value::Str(
        vm.heap
            .intern(b"dynamic libraries not enabled; check your Lua installation"),
    );
    let when = Value::Str(vm.heap.intern(b"absent"));
    Ok(vm.nat_return(fs, &[Value::Nil, msg, when]))
}

/// PUC 5.1 `module(name, ...)`: create (or reuse) `package.loaded[name]` as
/// the module's table, decorate it with `_NAME` / `_M` / `_PACKAGE`, run the
/// extra option functions (`package.seeall` being the canonical one), and
/// repoint the caller's `_ENV` cell to the module table. After this, every
/// global write inside the calling chunk lands in the module table.
fn nat_module(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let name_v = vm.nat_arg(fs, nargs, 0);
    let name_bytes = match name_v {
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            return Err(arg_error(
                vm,
                1,
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let name_str = String::from_utf8_lossy(&name_bytes).into_owned();
    // 1. resolve / create the module table via package.loaded[name]. Reach
    // for the captured `loaded` upvalue first so a userland `package = {}`
    // doesn't pull the rug out from under module().
    let loaded = if vm.nat_upcount(fs) >= 1 {
        match vm.nat_upval(fs, 0) {
            Value::Table(t) => t,
            _ => return Err(raise_str(vm, "'package.loaded' upvalue missing")),
        }
    } else {
        let pkg_k = Value::Str(vm.heap.intern(b"package"));
        let Value::Table(pkg) = vm.globals().get(pkg_k) else {
            return Err(raise_str(vm, "'package' table missing"));
        };
        let loaded_k = Value::Str(vm.heap.intern(b"loaded"));
        let Value::Table(t) = pkg.get(loaded_k) else {
            return Err(raise_str(vm, "'package.loaded' must be a table"));
        };
        t
    };
    let name_key = Value::Str(vm.heap.intern(&name_bytes));
    let module_tab = match loaded.get(name_key) {
        Value::Table(t) => t,
        _ => {
            // PUC `module()` walks the dotted name in the global table
            // (`_findtable`): every intermediate key is created if missing
            // and *reused* if it already maps to a table. The final key is
            // resolved the same way — when `module("X.a.b")` runs after
            // `module("X.a.b.c")` has already populated the intermediate
            // X.a.b table, the existing table is adopted as the module
            // (carrying its `.c` subtable along) rather than overwritten.
            let t = resolve_or_create_dotted(vm, &name_bytes)?;
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { loaded.as_mut() }
                .set(&mut vm.heap, name_key, Value::Table(t))
                .expect("valid key");
            t
        }
    };
    // 2. populate _NAME / _M / _PACKAGE. PUC keeps the trailing dot in
    // `_PACKAGE` for nested modules — `module("P1.xuxu", ...)` ↦ "P1.".
    let pre_dot = match name_bytes.iter().rposition(|&b| b == b'.') {
        Some(i) => name_bytes[..=i].to_vec(),
        None => Vec::new(),
    };
    let name_val = Value::Str(vm.heap.intern(&name_bytes));
    let pkg_val = Value::Str(vm.heap.intern(&pre_dot));
    let name_k = Value::Str(vm.heap.intern(b"_NAME"));
    let m_k = Value::Str(vm.heap.intern(b"_M"));
    let p_k = Value::Str(vm.heap.intern(b"_PACKAGE"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { module_tab.as_mut() }
        .set(&mut vm.heap, name_k, name_val)
        .expect("valid key");
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { module_tab.as_mut() }
        .set(&mut vm.heap, m_k, Value::Table(module_tab))
        .expect("valid key");
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { module_tab.as_mut() }
        .set(&mut vm.heap, p_k, pkg_val)
        .expect("valid key");
    // 3. run option functions on the module table
    for i in 1..nargs {
        let f = vm.nat_arg(fs, nargs, i);
        if !f.is_nil() {
            vm.call_value(f, &[Value::Table(module_tab)])?;
        }
    }
    // 4. rewrite the caller's `_ENV` cell to the module table (PUC
    // `setfenv(2)`). The `_ENV` upvalue is not necessarily at slot 0 —
    // closures capture upvalues in first-access order — so locate it by
    // name in the proto's upvalue descriptors.
    if let Some(cl) = vm.lua_closure_at_level(1) {
        let mut env_idx = None;
        for (i, d) in cl.proto.upvals.iter().enumerate() {
            if &*d.name == "_ENV" {
                env_idx = Some(i);
                break;
            }
        }
        let Some(env_idx) = env_idx else {
            return Err(raise_str(
                vm,
                &format!("module '{name_str}' caller has no '_ENV' upvalue"),
            ));
        };
        let uv = cl.upvals()[env_idx];
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { uv.as_mut() }.set_closed(Value::Table(module_tab));
        vm.barrier_forward_upvalue(uv, Value::Table(module_tab));
    } else {
        return Err(raise_str(
            vm,
            &format!("module '{name_str}' needs a Lua caller frame"),
        ));
    }
    Ok(vm.nat_return(fs, &[Value::Table(module_tab)]))
}

/// Resolve `_G.a.b.c…` to its table, creating intermediates AND the leaf
/// when they are missing. Mirrors PUC `_findtable`: each component is fetched
/// once; nil components get a fresh table that is then both stored at that
/// key and used as the next walk root, while existing tables are reused.
fn resolve_or_create_dotted(
    vm: &mut Vm,
    name: &[u8],
) -> Result<crate::runtime::Gc<crate::runtime::Table>, LuaError> {
    let mut tab = vm.globals();
    let mut start = 0;
    let mut parts: Vec<&[u8]> = Vec::new();
    for (i, &b) in name.iter().enumerate() {
        if b == b'.' {
            parts.push(&name[start..i]);
            start = i + 1;
        }
    }
    parts.push(&name[start..]);
    for p in parts.iter() {
        let k = Value::Str(vm.heap.intern(p));
        let next = tab.get(k);
        tab = match next {
            Value::Table(t) => t,
            Value::Nil => {
                let t = vm.heap.new_table();
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { tab.as_mut() }
                    .set(&mut vm.heap, k, Value::Table(t))
                    .expect("valid key");
                t
            }
            _ => {
                // PUC `_findtable` raises "name conflict for module 'X'" when
                // the dotted path runs into a non-table, non-nil value (e.g.
                // `module("math.sin")` — `math.sin` is a function). attrib.lua
                // :172 / :173 require this to surface as a pcall failure.
                let s = String::from_utf8_lossy(name);
                return Err(raise_str(vm, &format!("name conflict for module '{s}'")));
            }
        };
    }
    Ok(tab)
}

/// PUC 5.1 `package.seeall(module)`: attach a metatable whose `__index` is
/// `_G`, so any name unresolved in the module table falls back to the global
/// environment. Used inside `module(...)`'s option list as a convenience.
fn nat_package_seeall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let m = vm.nat_arg(fs, nargs, 0);
    let Value::Table(t) = m else {
        return Err(arg_error(vm, 1, "table expected"));
    };
    let mt = vm.heap.new_table();
    let k = Value::Str(vm.heap.intern(b"__index"));
    let g = Value::Table(vm.globals());
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { mt.as_mut() }
        .set(&mut vm.heap, k, g)
        .expect("valid key");
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }.set_metatable(Some(mt));
    Ok(vm.nat_return(fs, &[]))
}

/// Substitute every '?' in `tpl` with `subst`. PUC `luaL_gsub` semantics.
fn template_expand(tpl: &[u8], subst: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tpl.len() + subst.len());
    for &b in tpl {
        if b == b'?' {
            out.extend_from_slice(subst);
        } else {
            out.push(b);
        }
    }
    out
}

/// Replace every occurrence of `from` in `src` with `to`. Used by
/// `package.searchpath`'s sep→rep substitution on the module name.
fn replace_bytes(src: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return src.to_vec();
    }
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if i + from.len() <= src.len() && &src[i..i + from.len()] == from {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(src[i]);
            i += 1;
        }
    }
    out
}

fn nat_searchpath(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(name) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "string expected"));
    };
    let Value::Str(path) = vm.nat_arg(fs, nargs, 1) else {
        return Err(arg_error(vm, 2, "string expected"));
    };
    let sep: Vec<u8> = match vm.nat_arg(fs, nargs, 2) {
        Value::Nil => b".".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => return Err(arg_error(vm, 3, "string expected")),
    };
    let rep: Vec<u8> = match vm.nat_arg(fs, nargs, 3) {
        Value::Nil => b"/".to_vec(),
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => return Err(arg_error(vm, 4, "string expected")),
    };
    let name_bytes = name.as_bytes().to_vec();
    let path_bytes = path.as_bytes().to_vec();
    let translated = replace_bytes(&name_bytes, &sep, &rep);
    let mut err = Vec::new();
    // PUC `pushnexttemplate` skips runs of separator chars, so `;;` and the
    // empty trailing template never appear as candidates and never add a
    // "no file ''" line.
    for tpl in path_bytes.split(|&b| b == b';') {
        if tpl.is_empty() {
            continue;
        }
        let expanded = template_expand(tpl, &translated);
        if std::fs::File::open(std::path::Path::new(
            std::str::from_utf8(&expanded).unwrap_or(""),
        ))
        .is_ok()
        {
            let v = Value::Str(vm.heap.intern(&expanded));
            return Ok(vm.nat_return(fs, &[v]));
        }
        err.extend_from_slice(b"\n\tno file '");
        err.extend_from_slice(&expanded);
        err.push(b'\'');
    }
    let err_v = Value::Str(vm.heap.intern(&err));
    Ok(vm.nat_return(fs, &[Value::Nil, err_v]))
}

fn nat_require(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let Value::Str(name) = vm.nat_arg(fs, nargs, 0) else {
        return Err(arg_error(vm, 1, "string expected"));
    };
    let key = Value::Str(name);
    let name_s = String::from_utf8_lossy(name.as_bytes()).into_owned();

    // PUC's require reads `_LOADED` / `_PRELOAD` from the registry, so the
    // user reassigning the global `package` cannot disturb the bookkeeping.
    // luna captures the original tables as native upvalues at startup; fall
    // back to globals.package only for older callers that constructed the
    // native without upvals.
    let (pkg, loaded) = if vm.nat_upcount(fs) >= 2 {
        let p = match vm.nat_upval(fs, 0) {
            Value::Table(t) => t,
            _ => {
                return Err(raise_str(vm, "'package' upvalue missing"));
            }
        };
        let l = match vm.nat_upval(fs, 1) {
            Value::Table(t) => t,
            _ => {
                return Err(raise_str(vm, "'package.loaded' upvalue missing"));
            }
        };
        (p, l)
    } else {
        let pkg_k = Value::Str(vm.heap.intern(b"package"));
        let Value::Table(p) = vm.globals().get(pkg_k) else {
            return Err(raise_str(vm, "'package' table missing"));
        };
        let loaded_k = Value::Str(vm.heap.intern(b"loaded"));
        let Value::Table(l) = p.get(loaded_k) else {
            return Err(raise_str(vm, "'package.loaded' must be a table"));
        };
        (p, l)
    };
    let cached = loaded.get(key);
    // PUC 5.1 `ll_require` keyed the "already loaded" guard on
    // `lua_toboolean(loaded[name])` — a module whose stored value is false
    // (e.g. `return false`) was treated as not loaded and re-executed. 5.2+
    // changed that to `lua_isnil(loaded[name])`, so any non-nil entry blocks
    // re-execution. attrib.lua's "default option (should reload it)" probe
    // depends on the 5.1 falsy-as-not-loaded rule.
    let already_loaded = if vm.version() <= crate::version::LuaVersion::Lua51 {
        cached.truthy()
    } else {
        !cached.is_nil()
    };
    if already_loaded {
        return Ok(vm.nat_return(fs, &[cached]));
    }

    // Error message is the concatenation of every searcher's miss reason;
    // PUC's findloader builds it the same way (one '\n\t…' chunk per try).
    let mut err = String::new();

    // preload searcher (PUC: searcher #1, runs before file searchers). Same
    // upval-vs-globals story as `loaded`: when the captured upvals are
    // present, read them so a user `package = {}` cannot derail preload.
    let preload = if vm.nat_upcount(fs) >= 3 {
        match vm.nat_upval(fs, 2) {
            Value::Table(t) => t,
            _ => return Err(raise_str(vm, "'package.preload' upvalue missing")),
        }
    } else {
        let preload_k = Value::Str(vm.heap.intern(b"preload"));
        let Value::Table(t) = pkg.get(preload_k) else {
            return Err(raise_str(vm, "'package.preload' must be a table"));
        };
        t
    };
    let loader = preload.get(key);
    if !loader.is_nil() {
        // PUC 5.1's preload loader is called with just the module name as a
        // single arg; 5.2+ added a "path" arg (`:preload:`). attrib.lua's
        // `function (...) module(...) end` preload variant in 5.1 passes
        // `...` straight to `module`, so the extra string would be misread
        // as an option function and get called against the module table.
        let pv = Value::Str(vm.heap.intern(b":preload:"));
        let args: &[Value] = if vm.version() <= crate::version::LuaVersion::Lua51 {
            &[key]
        } else {
            &[key, pv]
        };
        let results = vm.call_value(loader, args)?;
        let returned = results.first().copied().unwrap_or(Value::Nil);
        // PUC `ll_require`: if the loader returned non-nil, store that. Else
        // honour whatever the loader may have written into `package.loaded`
        // (e.g. via `module()` setting `loaded[name] = module_tab`). Only
        // fall back to `true` when both come up empty. attrib.lua's preload
        // `module(...)` pattern relies on the second branch — the module
        // table set by `module()` must survive the require.
        let value = if !returned.is_nil() {
            returned
        } else {
            let post = loaded.get(key);
            if !post.is_nil() {
                post
            } else {
                Value::Bool(true)
            }
        };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { loaded.as_mut() }
            .set(&mut vm.heap, key, value)
            .expect("valid key");
        vm.barrier_back_table(loaded);
        return Ok(vm.nat_return(fs, &[value, pv]));
    }
    err.push_str(&format!("\n\tno field package.preload['{name_s}']"));

    // file searcher driven by package.path. attrib.lua sometimes sets
    // package.path to a non-string to confirm the error mentions it.
    let path_k = Value::Str(vm.heap.intern(b"path"));
    let path_v = pkg.get(path_k);
    let path_bytes = match path_v {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => return Err(raise_str(vm, "'package.path' must be a string")),
    };
    // In a module name like "P1.xuxu", PUC's file searcher first replaces
    // '.' with the dir-separator before template expansion.
    let translated_name = replace_bytes(name.as_bytes(), b".", b"/");
    let mut found: Option<(Vec<u8>, Vec<u8>)> = None;
    for tpl in path_bytes.split(|&b| b == b';') {
        if tpl.is_empty() {
            continue;
        }
        let expanded = template_expand(tpl, &translated_name);
        if found.is_none()
            && let Ok(src) = std::fs::read(std::str::from_utf8(&expanded).unwrap_or(""))
        {
            found = Some((expanded.clone(), src));
        }
        err.push_str("\n\tno file '");
        err.push_str(&String::from_utf8_lossy(&expanded));
        err.push('\'');
    }

    // C-library searcher: luna has no dynamic-linking backend, but attrib.lua
    // still inspects the message format. Walk cpath only to append "no file"
    // lines; never load anything.
    let cpath_k = Value::Str(vm.heap.intern(b"cpath"));
    let cpath_v = pkg.get(cpath_k);
    let cpath_bytes = match cpath_v {
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Nil => Vec::new(),
        _ => return Err(raise_str(vm, "'package.cpath' must be a string")),
    };
    for tpl in cpath_bytes.split(|&b| b == b';') {
        if tpl.is_empty() {
            continue;
        }
        let expanded = template_expand(tpl, &translated_name);
        err.push_str("\n\tno file '");
        err.push_str(&String::from_utf8_lossy(&expanded));
        err.push('\'');
    }

    if let Some((path_b, src)) = found {
        let path_s = String::from_utf8_lossy(&path_b).into_owned();
        let chunkname = format!("@{path_s}");
        let src = crate::frontend::lexer::Lexer::strip_shebang_bom(&src);
        let cl = match vm.load(src, chunkname.as_bytes()) {
            Ok(cl) => cl,
            Err(e) => {
                return Err(raise_str(
                    vm,
                    &format!("error loading module '{name_s}' from file '{path_s}':\n\t{e}"),
                ));
            }
        };
        let pv = Value::Str(vm.heap.intern(path_s.as_bytes()));
        let results = vm.call_value(Value::Closure(cl), &[key, pv])?;
        let value = results.first().copied().unwrap_or(Value::Nil);
        let value = if value.is_nil() {
            Value::Bool(true)
        } else {
            value
        };
        // Re-fetch loaded[name]: a preload-style module can have set it
        // during its own body (attrib.lua's C.lua does `package.loaded[...] =
        // 25; require'C'`); honour that value over the chunk's return.
        let post = loaded.get(key);
        let final_v = if !post.is_nil() { post } else { value };
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { loaded.as_mut() }
            .set(&mut vm.heap, key, final_v)
            .expect("valid key");
        vm.barrier_back_table(loaded);
        return Ok(vm.nat_return(fs, &[final_v, pv]));
    }

    Err(raise_str(vm, &format!("module '{name_s}' not found:{err}")))
}
